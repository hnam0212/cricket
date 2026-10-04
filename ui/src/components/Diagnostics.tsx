import type { Status } from "../api";
import { activityText, sourceName, strings, tabTitle } from "../strings";
import { PeakBar } from "./PeakBar";

interface DiagnosticsProps {
  status: Status;
}

/** Shows what Cricket sees, so the user can tell why the music paused. */
export function Diagnostics({ status }: DiagnosticsProps) {
  // Newest first, so the latest decision is visible without scrolling.
  const events = [...status.events].reverse();
  const { source } = status;
  const isSourceApp = (app: string) => source?.kind === "app" && source.app === app;
  const isSourceTab = (id: number) => source?.kind === "tab" && source.id === id;

  return (
    <section className="diagnostics">
      <h3>{strings.diagnosticsEngine}</h3>
      <dl>
        <dt>{strings.engineState}</dt>
        <dd>{status.state}</dd>
        <dt>{strings.engineReason}</dt>
        <dd>{status.reason || strings.none}</dd>
        <dt>{strings.engineSource}</dt>
        <dd>{status.source ? sourceName(status.source, status.tabs) : strings.none}</dd>
        <dt>{strings.enginePlayback}</dt>
        <dd>{status.playback ?? strings.unavailable}</dd>
        <dt>{strings.engineVolume}</dt>
        <dd>{status.volume === null ? strings.unavailable : status.volume.toFixed(3)}</dd>
        <dt>{strings.engineActivity}</dt>
        <dd>{activityText(status.activity)}</dd>
        <dt>{strings.engineExtension}</dt>
        <dd>
          {status.bridge.error ??
            (status.bridge.connected
              ? strings.extensionConnected
              : strings.extensionNotConnected)}
        </dd>
        <dt>{strings.microphone}</dt>
        <dd>
          {status.mic_users.length > 0
            ? strings.micInUseBy(status.mic_users)
            : strings.micNotInUse}
        </dd>
      </dl>

      {status.errors.length > 0 && (
        <>
          <h3>{strings.errors}</h3>
          <ul className="errors">
            {status.errors.map((error) => (
              <li key={error}>{error}</li>
            ))}
          </ul>
        </>
      )}

      <h3>{strings.diagnosticsSessions}</h3>
      <table>
        <thead>
          <tr>
            <th>{strings.colApp}</th>
            <th>{strings.colPeak}</th>
            <th>{strings.colState}</th>
            <th>{strings.colSessions}</th>
          </tr>
        </thead>
        <tbody>
          {status.apps.map((app) => (
            <tr key={app.app} className={isSourceApp(app.app) ? "is-source" : undefined}>
              <td>
                {app.app}
                {app.is_system_sounds && ` (${strings.systemSounds})`}
              </td>
              <td>
                <PeakBar peak={app.peak} makingSound={app.making_sound} /> {app.peak.toFixed(3)}
              </td>
              <td>{app.active ? strings.active : strings.inactive}</td>
              <td>{app.sessions}</td>
            </tr>
          ))}
        </tbody>
      </table>

      {status.tabs.length > 0 && (
        <>
          <h3>{strings.diagnosticsTabs}</h3>
          <table>
            <thead>
              <tr>
                <th>{strings.colTab}</th>
                <th>{strings.colAudible}</th>
              </tr>
            </thead>
            <tbody>
              {status.tabs.map((tab) => (
                <tr key={tab.id} className={isSourceTab(tab.id) ? "is-source" : undefined}>
                  <td>{tabTitle(tab)}</td>
                  <td>{tab.audible ? strings.yes : strings.no}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </>
      )}

      <h3>{strings.diagnosticsLog}</h3>
      {events.length === 0 ? (
        <p className="hint">{strings.noEvents}</p>
      ) : (
        <ol className="log">
          {events.map((event, index) => (
            <li key={`${event.at}-${index}`}>
              <span className="log-time">{event.at.toFixed(2)}s</span> {event.text}
            </li>
          ))}
        </ol>
      )}
    </section>
  );
}
