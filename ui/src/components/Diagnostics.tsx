import type { Status } from "../api";
import { activityText, strings } from "../strings";
import { PeakBar } from "./PeakBar";

interface DiagnosticsProps {
  status: Status;
}

/** Shows what Cricket sees, so the user can tell why the music paused. */
export function Diagnostics({ status }: DiagnosticsProps) {
  // Newest first, so the latest decision is visible without scrolling.
  const events = [...status.events].reverse();

  return (
    <section className="diagnostics">
      <h3>{strings.diagnosticsEngine}</h3>
      <dl>
        <dt>{strings.engineState}</dt>
        <dd>{status.state}</dd>
        <dt>{strings.engineReason}</dt>
        <dd>{status.reason || strings.none}</dd>
        <dt>{strings.enginePlayback}</dt>
        <dd>{status.playback ?? strings.unavailable}</dd>
        <dt>{strings.engineVolume}</dt>
        <dd>{status.volume === null ? strings.unavailable : status.volume.toFixed(3)}</dd>
        <dt>{strings.engineActivity}</dt>
        <dd>{activityText(status.activity)}</dd>
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
            <tr key={app.app} className={app.app === status.source ? "is-source" : undefined}>
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
