import { useEffect, useState } from "react";
import { api, type AppConfig, type Status } from "./api";
import { Diagnostics } from "./components/Diagnostics";
import { ExtensionPanel } from "./components/ExtensionPanel";
import { SettingsPanel } from "./components/SettingsPanel";
import { SourcePicker } from "./components/SourcePicker";
import { Switch } from "./components/Switch";
import { statusText, strings } from "./strings";

/** How often the window asks the backend for the latest status. */
const STATUS_INTERVAL_MS = 200;

export function App() {
  const [status, setStatus] = useState<Status | null>(null);
  const [config, setConfig] = useState<AppConfig | null>(null);
  const [version, setVersion] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const fail = (reason: unknown) => setError(String(reason));
    api.getConfig().then(setConfig).catch(fail);
    api.coreInfo().then(setVersion).catch(fail);

    const poll = () => api.getStatus().then(setStatus).catch(fail);
    poll();
    const timer = window.setInterval(poll, STATUS_INTERVAL_MS);
    return () => window.clearInterval(timer);
  }, []);

  if (error) {
    return (
      <main>
        <p role="alert">
          {strings.loadError} {error}
        </p>
      </main>
    );
  }
  if (!status || !config) {
    return (
      <main>
        <p>{strings.loading}</p>
      </main>
    );
  }

  // Every change goes to the backend, which saves it and returns the
  // configuration it now holds.
  const apply = (change: Promise<AppConfig>) => {
    change.then(setConfig).catch((reason: unknown) => setError(String(reason)));
  };

  const { headline, detail } = statusText(status);

  return (
    <main className={config.mini_mode ? "mini" : undefined}>
      <header>
        <h1>{strings.appName}</h1>
        <button
          type="button"
          className="link"
          onClick={() => apply(api.setMiniMode(!config.mini_mode))}
        >
          {config.mini_mode ? strings.miniModeOff : strings.miniModeOn}
        </button>
      </header>

      <section className={`status status-${status.state}`} aria-live="polite">
        <div className="status-text">
          <p className="status-headline">{headline}</p>
          <p className="status-detail">{detail}</p>
        </div>
        <Switch
          checked={status.enabled}
          label={status.enabled ? strings.enabledLabel : strings.disabledLabel}
          onChange={(enabled) => apply(api.setEnabled(enabled))}
        />
      </section>

      {!config.mini_mode && (
        <>
          <SourcePicker
            apps={status.apps}
            tabs={status.tabs}
            bridge={status.bridge}
            source={status.source}
            onPick={(source) => apply(api.setSource(source))}
          />

          <SettingsPanel
            settings={config.settings}
            minimizeToTray={config.minimize_to_tray}
            onChange={(settings) => apply(api.setSettings(settings))}
            onMinimizeToTray={(value) => apply(api.setMinimizeToTray(value))}
          />

          <ExtensionPanel
            bridge={status.bridge}
            token={config.bridge_token}
            browserOverride={config.browser_override}
            onBrowserOverride={(value) => apply(api.setBrowserOverride(value))}
          />

          <label className="check">
            <input
              type="checkbox"
              checked={config.show_diagnostics}
              onChange={(event) => apply(api.setShowDiagnostics(event.target.checked))}
            />
            {strings.diagnosticsToggle}
          </label>
          {config.show_diagnostics && <Diagnostics status={status} />}

          <footer>{version}</footer>
        </>
      )}
    </main>
  );
}
