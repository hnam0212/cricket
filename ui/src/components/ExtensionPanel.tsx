import { useState } from "react";
import type { BridgeStatus } from "../api";
import { strings } from "../strings";

interface ExtensionPanelProps {
  bridge: BridgeStatus;
  token: string;
  /** Browser executable chosen by the user, if any. */
  browserOverride: string | null;
  onBrowserOverride: (value: string | null) => void;
}

/** Chromium browsers the extension can run in, by executable. */
const BROWSERS: { exe: string; name: string }[] = [
  { exe: "chrome.exe", name: "Google Chrome" },
  { exe: "msedge.exe", name: "Microsoft Edge" },
  { exe: "brave.exe", name: "Brave" },
  { exe: "vivaldi.exe", name: "Vivaldi" },
  { exe: "opera.exe", name: "Opera" },
  { exe: "chromium.exe", name: "Chromium" },
];

/** Pairing details for the browser extension. */
export function ExtensionPanel({
  bridge,
  token,
  browserOverride,
  onBrowserOverride,
}: ExtensionPanelProps) {
  const [copied, setCopied] = useState(false);

  const copy = () => {
    navigator.clipboard
      .writeText(token)
      .then(() => {
        setCopied(true);
        window.setTimeout(() => setCopied(false), 1500);
      })
      // Clipboard access can be refused; the token is selectable anyway.
      .catch(() => undefined);
  };

  const state = bridge.error
    ? strings.extensionFailed(bridge.port, bridge.error)
    : bridge.connected
      ? strings.extensionConnected
      : strings.extensionNotConnected;

  return (
    <details className="panel">
      <summary>
        {strings.extensionHeading}
        <span className={bridge.connected ? "badge badge-on" : "badge"}>{state}</span>
      </summary>

      <p className="hint">{strings.extensionIntro}</p>
      <ol className="steps">
        {strings.extensionSteps.map((step) => (
          <li key={step}>{step}</li>
        ))}
      </ol>

      <div className="field">
        <span className="field-label">{strings.extensionToken}</span>
        <button type="button" onClick={copy}>
          {copied ? strings.copied : strings.copy}
        </button>
        <code className="token">{token}</code>
      </div>
      <div className="field">
        <label className="field-label" htmlFor="browser-select">
          {strings.extensionBrowser}
        </label>
        <select
          id="browser-select"
          value={browserOverride ?? ""}
          onChange={(event) => onBrowserOverride(event.target.value || null)}
        >
          <option value="">
            {bridge.browser && bridge.browser_source !== "chosen"
              ? `${strings.extensionBrowserAuto} (${bridge.browser})`
              : strings.extensionBrowserAuto}
          </option>
          {BROWSERS.map((browser) => (
            <option key={browser.exe} value={browser.exe}>
              {browser.name}
            </option>
          ))}
          {browserOverride && !BROWSERS.some((browser) => browser.exe === browserOverride) && (
            <option value={browserOverride}>{browserOverride}</option>
          )}
        </select>
        <p className="hint">{strings.extensionBrowserWhy}</p>
      </div>
      <div className="field">
        <span className="field-label">{strings.extensionPort}</span>
        <code>{bridge.port}</code>
      </div>
    </details>
  );
}
