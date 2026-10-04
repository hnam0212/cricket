import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { strings } from "./strings";

type CoreInfo =
  | { status: "loading" }
  | { status: "ok"; text: string }
  | { status: "error"; message: string };

export function App() {
  const [core, setCore] = useState<CoreInfo>({ status: "loading" });

  useEffect(() => {
    invoke<string>("core_info")
      .then((text) => setCore({ status: "ok", text }))
      .catch((err: unknown) => setCore({ status: "error", message: String(err) }));
  }, []);

  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: "2rem" }}>
      <h1>{strings.appName}</h1>
      {core.status === "loading" && <p>{strings.coreLoading}</p>}
      {core.status === "ok" && (
        <p>
          {strings.coreLabel} <code>{core.text}</code>
        </p>
      )}
      {core.status === "error" && (
        <p role="alert">
          {strings.coreError} {core.message}
        </p>
      )}
    </main>
  );
}
