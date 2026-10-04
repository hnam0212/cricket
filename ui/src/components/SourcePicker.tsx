import type { AppView } from "../api";
import { strings } from "../strings";
import { PeakBar } from "./PeakBar";

interface SourcePickerProps {
  apps: AppView[];
  source: string | null;
  onPick: (source: string | null) => void;
}

export function SourcePicker({ apps, source, onPick }: SourcePickerProps) {
  const candidates = apps.filter((app) => !app.is_system_sounds);
  // Keep the chosen source in the list even while it has no audio session
  // (a paused player often has none), so the choice stays visible.
  const sourceMissing = source !== null && !candidates.some((app) => app.app === source);

  return (
    <section>
      <h2>{strings.sourceHeading}</h2>
      <p className="hint">{strings.sourceHint}</p>
      <ul className="sources">
        <li>
          <label className="source">
            <input
              type="radio"
              name="source"
              checked={source === null}
              onChange={() => onPick(null)}
            />
            <span className="source-name">{strings.sourceNone}</span>
          </label>
        </li>
        {sourceMissing && (
          <li>
            <label className="source">
              <input type="radio" name="source" checked readOnly />
              <span className="source-name">{source}</span>
              <span className="source-note">{strings.sourceNotRunning}</span>
            </label>
          </li>
        )}
        {candidates.map((app) => (
          <li key={app.app}>
            <label className="source">
              <input
                type="radio"
                name="source"
                checked={source === app.app}
                onChange={() => onPick(app.app)}
              />
              <span className="source-name">{app.app}</span>
              <PeakBar peak={app.peak} makingSound={app.making_sound} />
              <span className="source-note">
                {app.making_sound ? strings.makingSound : strings.silent}
              </span>
            </label>
          </li>
        ))}
      </ul>
      {candidates.length === 0 && !sourceMissing && <p className="hint">{strings.sourceEmpty}</p>}
    </section>
  );
}
