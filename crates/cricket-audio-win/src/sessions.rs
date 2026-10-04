use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cricket_core::audio::{
    group_sessions, ActivitySnapshot, AppId, AudioBackend, BackendError, BackendResult, SessionInfo,
};
use windows::core::{implement, Interface, Ref, PWSTR};
use windows::Win32::Foundation::{CloseHandle, S_OK};
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    eCapture, eRender, AudioSessionStateActive, AudioSessionStateExpired, EDataFlow,
    IAudioSessionControl, IAudioSessionControl2, IAudioSessionManager2, IAudioSessionNotification,
    IAudioSessionNotification_Impl, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
};

use crate::com::{os_error, ComGuard};

/// Sessions are also re-enumerated on a timer. Notifications cover new
/// sessions, the timer covers what they do not: devices being plugged in or
/// removed, and any notification that got lost.
const FULL_REFRESH_INTERVAL: Duration = Duration::from_secs(2);

const SYSTEM_SOUNDS_APP: &str = "system sounds";

/// WASAPI implementation of [`AudioBackend`].
///
/// Holds COM objects: create it and use it on one thread.
pub struct WinAudioBackend {
    enumerator: IMMDeviceEnumerator,
    render: Vec<Endpoint>,
    capture: Vec<Endpoint>,
    /// Set by the session notification callback, on a system thread.
    sessions_changed: Arc<AtomicBool>,
    last_refresh: Instant,
    own_pid: u32,
    // Last field, so it is dropped after every COM object above.
    _com: ComGuard,
}

/// One audio device and the sessions that were on it at the last refresh.
struct Endpoint {
    manager: IAudioSessionManager2,
    notifier: IAudioSessionNotification,
    sessions: Vec<Session>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // Ignore the result: the device may already be gone.
        let _ = unsafe { self.manager.UnregisterSessionNotification(&self.notifier) };
    }
}

struct Session {
    control: IAudioSessionControl2,
    meter: Option<IAudioMeterInformation>,
    volume: Option<ISimpleAudioVolume>,
    pid: u32,
    app: AppId,
    is_system_sounds: bool,
}

#[implement(IAudioSessionNotification)]
struct SessionCreated {
    sessions_changed: Arc<AtomicBool>,
}

impl IAudioSessionNotification_Impl for SessionCreated_Impl {
    fn OnSessionCreated(
        &self,
        _new_session: Ref<IAudioSessionControl>,
    ) -> windows::core::Result<()> {
        // Runs on a system thread. Only raise the flag; the polling thread
        // re-enumerates on its next snapshot.
        self.sessions_changed.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl WinAudioBackend {
    pub fn new() -> BackendResult<Self> {
        let com = ComGuard::new()?;
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
                .map_err(|e| os_error("create device enumerator", &e))?;
        let mut backend = Self {
            enumerator,
            render: Vec::new(),
            capture: Vec::new(),
            sessions_changed: Arc::new(AtomicBool::new(false)),
            last_refresh: Instant::now(),
            own_pid: std::process::id(),
            _com: com,
        };
        backend.refresh()?;
        Ok(backend)
    }

    /// Re-enumerates all active devices and their sessions.
    fn refresh(&mut self) -> BackendResult<()> {
        // Clear the flag first: a session created while we enumerate raises
        // it again and triggers one more refresh, instead of being lost.
        self.sessions_changed.store(false, Ordering::Relaxed);
        self.render = self.enumerate(eRender)?;
        self.capture = self.enumerate(eCapture)?;
        self.last_refresh = Instant::now();
        Ok(())
    }

    fn refresh_if_needed(&mut self) -> BackendResult<()> {
        if self.sessions_changed.load(Ordering::Relaxed)
            || self.last_refresh.elapsed() >= FULL_REFRESH_INTERVAL
        {
            self.refresh()?;
        }
        Ok(())
    }

    fn enumerate(&self, flow: EDataFlow) -> BackendResult<Vec<Endpoint>> {
        let devices = unsafe {
            self.enumerator
                .EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)
        }
        .map_err(|e| os_error("enumerate audio endpoints", &e))?;
        let count =
            unsafe { devices.GetCount() }.map_err(|e| os_error("count audio endpoints", &e))?;

        let mut endpoints = Vec::new();
        for index in 0..count {
            // A device can vanish between the count and the open (unplugged
            // headset). Skip it; the next refresh sees the new device list.
            if let Ok(endpoint) = self.open_endpoint(&devices, index) {
                endpoints.push(endpoint);
            }
        }
        Ok(endpoints)
    }

    fn open_endpoint(
        &self,
        devices: &windows::Win32::Media::Audio::IMMDeviceCollection,
        index: u32,
    ) -> windows::core::Result<Endpoint> {
        unsafe {
            let device = devices.Item(index)?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;

            let notifier: IAudioSessionNotification = SessionCreated {
                sessions_changed: Arc::clone(&self.sessions_changed),
            }
            .into();
            manager.RegisterSessionNotification(&notifier)?;

            // Windows only starts delivering session notifications after the
            // sessions were enumerated once, so register first, then list.
            let list = manager.GetSessionEnumerator()?;
            let mut sessions = Vec::new();
            for i in 0..list.GetCount()? {
                if let Ok(session) = list
                    .GetSession(i)
                    .and_then(|control| open_session(&control))
                {
                    sessions.push(session);
                }
            }

            Ok(Endpoint {
                manager,
                notifier,
                sessions,
            })
        }
    }

    fn render_sessions_of<'a>(&'a self, app: &'a AppId) -> impl Iterator<Item = &'a Session> {
        self.render
            .iter()
            .flat_map(|endpoint| endpoint.sessions.iter())
            .filter(move |session| &session.app == app && !is_expired(session))
    }
}

impl AudioBackend for WinAudioBackend {
    fn snapshot(&mut self) -> BackendResult<ActivitySnapshot> {
        self.refresh_if_needed()?;

        let mut outputs = Vec::new();
        for session in self
            .render
            .iter()
            .flat_map(|endpoint| endpoint.sessions.iter())
        {
            if session.pid == self.own_pid {
                continue;
            }
            // A failing call means the session just died; leave it out.
            let Ok(state) = (unsafe { session.control.GetState() }) else {
                continue;
            };
            if state == AudioSessionStateExpired {
                continue;
            }
            let peak = session
                .meter
                .as_ref()
                .and_then(|meter| unsafe { meter.GetPeakValue() }.ok())
                .unwrap_or(0.0);
            outputs.push(SessionInfo {
                app: session.app.clone(),
                pid: session.pid,
                active: state == AudioSessionStateActive,
                peak,
                is_system_sounds: session.is_system_sounds,
            });
        }

        let mut mic_users: Vec<AppId> = self
            .capture
            .iter()
            .flat_map(|endpoint| endpoint.sessions.iter())
            .filter(|session| session.pid != self.own_pid)
            .filter(|session| {
                unsafe { session.control.GetState() }
                    .is_ok_and(|state| state == AudioSessionStateActive)
            })
            .map(|session| session.app.clone())
            .collect();
        mic_users.sort();
        mic_users.dedup();

        Ok(ActivitySnapshot {
            apps: group_sessions(&outputs),
            mic_users,
        })
    }

    fn volume(&mut self, app: &AppId) -> BackendResult<f32> {
        self.refresh_if_needed()?;
        self.render_sessions_of(app)
            .filter_map(|session| session.volume.as_ref())
            .filter_map(|volume| unsafe { volume.GetMasterVolume() }.ok())
            .reduce(f32::max)
            .ok_or_else(|| BackendError::AppNotFound(app.clone()))
    }

    fn set_volume(&mut self, app: &AppId, volume: f32) -> BackendResult<()> {
        self.refresh_if_needed()?;
        let level = volume.clamp(0.0, 1.0);
        let mut found = false;
        for control in self
            .render_sessions_of(app)
            .filter_map(|session| session.volume.as_ref())
        {
            // Null event context: we do not listen for volume change events,
            // so there is nothing to tell our own changes apart from.
            unsafe { control.SetMasterVolume(level, std::ptr::null()) }
                .map_err(|e| os_error("set session volume", &e))?;
            found = true;
        }
        if found {
            Ok(())
        } else {
            Err(BackendError::AppNotFound(app.clone()))
        }
    }
}

fn is_expired(session: &Session) -> bool {
    !unsafe { session.control.GetState() }.is_ok_and(|state| state != AudioSessionStateExpired)
}

fn open_session(control: &IAudioSessionControl) -> windows::core::Result<Session> {
    let control: IAudioSessionControl2 = control.cast()?;
    let pid = unsafe { control.GetProcessId() }.unwrap_or(0);
    // S_OK means "yes"; S_FALSE means "no".
    let is_system_sounds = unsafe { control.IsSystemSoundsSession() } == S_OK;
    let app = if is_system_sounds {
        AppId::new(SYSTEM_SOUNDS_APP)
    } else {
        process_name(pid).unwrap_or_else(|| AppId::new(&format!("pid-{pid}")))
    };
    Ok(Session {
        meter: control.cast().ok(),
        volume: control.cast().ok(),
        control,
        pid,
        app,
        is_system_sounds,
    })
}

/// Executable name of a process, or `None` if it cannot be opened (already
/// exited, or protected).
pub(crate) fn process_name(pid: u32) -> Option<AppId> {
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut len = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut len,
        );
        let _ = CloseHandle(handle);
        result.ok()?;

        let path = String::from_utf16_lossy(&buffer[..len as usize]);
        let name = Path::new(&path).file_name()?.to_str()?;
        Some(AppId::new(name))
    }
}
