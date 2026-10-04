import { useState } from "react";
import type { BridgeStatus } from "../api";
import { strings } from "../strings";

interface ExtensionPanelProps {
  bridge: BridgeStatus;
  token: string;
}

/** Pairing details for the browser extension. */
export function ExtensionPanel({ bridge, token }: ExtensionPanelProps) {
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
        <span className="field-label">{strings.extensionPort}</span>
        <code>{bridge.port}</code>
      </div>
    </details>
  );
}
