import { createRef } from "react";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import {
  MarkdownContent,
  MarkdownEditor,
  type MarkdownEditorHandle,
} from "@/components/Markdown";

describe("MarkdownContent", () => {
  it("renders the Markdown used in notes and summaries, including GFM", async () => {
    const { container } = render(
      <MarkdownContent>
        {"## Launch\n\n- [x] Ship **renderer**\n- [ ] Polish tables\n\n| Owner | Day |\n| --- | --- |\n| Pavel | Fri |"}
      </MarkdownContent>,
    );

    expect(await screen.findByRole("heading", { level: 2, name: "Launch" })).toBeInTheDocument();
    expect(screen.getByText("renderer").tagName).toBe("STRONG");
    expect(screen.getAllByRole("checkbox")).toHaveLength(2);
    expect(screen.getByRole("table")).toHaveTextContent("Pavel");
    expect(container).not.toHaveTextContent("## Launch");
  });

  it("does not interpret raw HTML from a note", async () => {
    const { container } = render(
      <MarkdownContent>{"before <script>window.bad = true</script> after"}</MarkdownContent>,
    );

    await screen.findByText(/window\.bad = true/);
    expect(container.querySelector("script")).toBeNull();
    expect(container).toHaveTextContent("<script>window.bad = true</script>");
  });
});

describe("MarkdownEditor", () => {
  it("shows rendered text at rest and edits the same source in place", async () => {
    const onChange = vi.fn();
    const onBlur = vi.fn();
    render(
      <MarkdownEditor
        value="**Important** note"
        onChange={onChange}
        onBlur={onBlur}
        placeholder="your notes..."
      />,
    );

    const textarea = screen.getByPlaceholderText("your notes...");
    expect(textarea).not.toBeVisible();
    expect((await screen.findByText("Important")).tagName).toBe("STRONG");

    fireEvent.click(screen.getByRole("button", { name: "Edit notes" }));
    await waitFor(() => expect(textarea).toHaveFocus());
    expect(textarea).toBeVisible();

    fireEvent.change(textarea, { target: { value: "# Updated" } });
    expect(onChange).toHaveBeenCalled();
    fireEvent.blur(textarea);
    expect(onBlur).toHaveBeenCalled();
  });

  it("exposes focus for the active-window Notes shortcut", async () => {
    const ref = createRef<MarkdownEditorHandle>();
    render(
      <MarkdownEditor
        ref={ref}
        value="Existing note"
        onChange={() => {}}
        onBlur={() => {}}
        placeholder="your notes..."
      />,
    );

    ref.current?.focus();
    await waitFor(() => expect(screen.getByPlaceholderText("your notes...")).toHaveFocus());
  });
});
