import { createRef } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SessionView, type SessionViewHandle } from "@/components/SessionView";
import * as ipc from "@/lib/ipc";
import type { SessionData } from "@/lib/ipc";

vi.mock("@/lib/ipc", () => ({
  saveSession: vi.fn().mockResolvedValue(undefined),
  updateActiveSession: vi.fn().mockResolvedValue(undefined),
}));

const baseData: SessionData = { title: "Weekly sync", date: "", notes: "", transcript: "" };

beforeEach(() => {
  vi.mocked(ipc.saveSession).mockClear();
  vi.mocked(ipc.updateActiveSession).mockClear();
});

describe("SessionView: stop-transition flush (Task 6 review fix)", () => {
  it("routes an unblurred, still-debouncing notes edit through update_active_session when imperatively flushed", async () => {
    // Reproduces the bug: App's Stop button can be clicked while the notes
    // textarea still has focus and its 500ms debounce timer hasn't fired
    // yet. App.handleStop must be able to force that edit out through the
    // *active* route (update_active_session) before it calls
    // stop_recording -- this is what SessionView's imperative
    // `flushPendingEdits` handle exists for.
    const ref = createRef<SessionViewHandle>();
    render(
      <SessionView
        ref={ref}
        path="/tmp/call.md"
        isActive
        initialData={baseData}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    const notes = screen.getByPlaceholderText("your notes...");
    fireEvent.change(notes, { target: { value: "call went well" } });
    // Deliberately no blur and no timer advance: the edit is still only
    // sitting in the debounce, exactly as it would be mid-keystroke.

    await ref.current?.flushPendingEdits();

    expect(ipc.updateActiveSession).toHaveBeenCalledWith(undefined, "call went well");
    expect(ipc.saveSession).not.toHaveBeenCalled();
  });

  it("does not re-flush a title already saved via blur (no redundant IPC call)", async () => {
    const ref = createRef<SessionViewHandle>();
    render(
      <SessionView
        ref={ref}
        path="/tmp/call.md"
        isActive
        initialData={baseData}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    const title = screen.getByPlaceholderText("Untitled");
    fireEvent.change(title, { target: { value: "New title" } });
    fireEvent.blur(title);
    expect(ipc.updateActiveSession).toHaveBeenCalledWith("New title", undefined);
    vi.mocked(ipc.updateActiveSession).mockClear();

    await ref.current?.flushPendingEdits();

    expect(ipc.updateActiveSession).not.toHaveBeenCalled();
  });
});

describe("SessionView: empty title normalization (Task 6 review fix)", () => {
  it("saves and displays 'Untitled' when the title is committed empty, for an inactive session", () => {
    render(
      <SessionView
        path="/tmp/call.md"
        isActive={false}
        initialData={{ ...baseData, title: "Old title" }}
        liveLines={[]}
        interimMe=""
        interimThem=""
      />
    );

    const title = screen.getByDisplayValue("Old title") as HTMLInputElement;
    fireEvent.change(title, { target: { value: "   " } });
    fireEvent.blur(title);

    expect(ipc.saveSession).toHaveBeenCalledWith("/tmp/call.md", "Untitled", "");
    expect(title.value).toBe("Untitled");
  });
});
