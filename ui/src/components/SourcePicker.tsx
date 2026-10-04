import type { AppView, BridgeStatus, Source, TabInfo } from "../api";
import { strings, tabTitle } from "../strings";
import { PeakBar } from "./PeakBar";

interface SourcePickerProps {
  apps: AppView[];
  tabs: TabInfo[];
  bridge: BridgeStatus;
  source: Source | null;
  /** The tab source predates a browser restart and must be picked again. */
  sourceStale: boolean;
  onPick: (source: Source | null) => void;
}

export function SourcePicker({
  apps,
  tabs,
  bridge,
  source,
  sourceStale,
  onPick,
}: SourcePickerProps) {
  const candidates = apps.filter((app) => !app.is_system_sounds);
  const sourceApp = source?.kind === "app" ? source.app : null;
  const sourceTab = source?.kind === "tab" ? source : null;
  // Keep the chosen source in the list even while it is not there (a paused
  // player often has no audio session; a tab may be closed or the extension
  // not connected), so the choice stays visible.
  const appMissing = sourceApp !== null && !candidates.some((app) => app.app === sourceApp);
  // A tab picked before the browser restarted keeps its number, which now
  // may be a different tab: never show that one as selected.
  const tabMissing =
    sourceTab !== null && (sourceStale || !tabs.some((tab) => tab.id === sourceTab.id));

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

        <li className="source-group">{strings.sourceApps}</li>
        {appMissing && (
          <li>
            <label className="source">
              <input type="radio" name="source" checked readOnly />
              <span className="source-name">{sourceApp}</span>
              <span className="source-note wide">{strings.sourceNotRunning}</span>
            </label>
          </li>
        )}
        {candidates.map((app) => (
          <li key={app.app}>
            <label className="source">
              <input
                type="radio"
                name="source"
                checked={sourceApp === app.app}
                onChange={() => onPick({ kind: "app", app: app.app })}
              />
              <span className="source-name">{app.app}</span>
              <PeakBar peak={app.peak} makingSound={app.making_sound} />
              <span className="source-note">
                {app.making_sound ? strings.makingSound : strings.silent}
              </span>
            </label>
          </li>
        ))}
        {candidates.length === 0 && !appMissing && (
          <li className="source-empty">{strings.sourceEmpty}</li>
        )}

        <li className="source-group">{strings.sourceTabs}</li>
        {tabMissing && sourceTab && (
          <li>
            <label className="source">
              <input type="radio" name="source" checked readOnly />
              <span className="source-name">{tabTitle(sourceTab)}</span>
              <span className="source-note wide">
                {sourceStale ? strings.tabStale : strings.tabNotAvailable}
              </span>
            </label>
          </li>
        )}
        {tabs.map((tab) => (
          <li key={tab.id}>
            <label
              className="source"
              title={tab.controllable ? tab.url : strings.tabNeedsPermissionHint}
            >
              <input
                type="radio"
                name="source"
                disabled={!tab.controllable}
                checked={!sourceStale && sourceTab?.id === tab.id}
                onChange={() => onPick({ kind: "tab", id: tab.id, title: tab.title })}
              />
              <span className="source-name">{tabTitle(tab)}</span>
              <span className={tab.audible ? "dot dot-on" : "dot"} aria-hidden="true" />
              <span className="source-note">
                {!tab.controllable
                  ? strings.tabNeedsPermission
                  : tab.audible
                    ? strings.makingSound
                    : strings.silent}
              </span>
            </label>
          </li>
        ))}
        {tabs.length === 0 && (
          <li className="source-empty">
            {bridge.connected ? strings.tabsEmpty : strings.tabsNotConnected}
          </li>
        )}
      </ul>
    </section>
  );
}
