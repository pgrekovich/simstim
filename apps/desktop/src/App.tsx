import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type Status =
  | { kind: "idle" }
  | { kind: "recording"; outputPath: string }
  | { kind: "error"; message: string };

function App() {
  const [status, setStatus] = useState<Status>({ kind: "idle" });
  const [lines, setLines] = useState<string[]>([]);
  const busyRef = useRef(false);

  useEffect(() => {
    const unlisten = listen<string>("transcript-line", (event) => {
      setLines((prev) => [...prev, event.payload]);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  async function handleToggle() {
    if (busyRef.current) return;
    busyRef.current = true;
    try {
      if (status.kind === "recording") {
        const path = await invoke<string>("stop_recording");
        setStatus({ kind: "idle" });
        void path;
      } else {
        setLines([]);
        const path = await invoke<string>("start_recording");
        setStatus({ kind: "recording", outputPath: path });
      }
    } catch (err) {
      setStatus({ kind: "error", message: String(err) });
    } finally {
      busyRef.current = false;
    }
  }

  const isRecording = status.kind === "recording";

  return (
    <main className="container">
      <button
        className={isRecording ? "toggle recording" : "toggle"}
        onClick={handleToggle}
      >
        {isRecording ? "Stop" : "Start"}
      </button>

      <p className={status.kind === "error" ? "status error" : "status"}>
        {status.kind === "idle" && "idle"}
        {status.kind === "recording" &&
          `recording -> ${status.outputPath}`}
        {status.kind === "error" && status.message}
      </p>

      <pre className="transcript">
        {lines.length > 0 ? lines.join("\n") : "..."}
      </pre>
    </main>
  );
}

export default App;
