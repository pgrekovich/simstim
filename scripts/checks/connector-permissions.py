#!/usr/bin/env python3
"""Opt-in Claude Code permission smoke test. Uses only a synthetic local MCP.
Requires an authenticated Claude CLI and consumes a small provider request.
No real connectors, notes, or credentials are read by this script.
"""
import json
import pathlib
import subprocess
import sys
import tempfile


def server(log):
    for line in sys.stdin:
        request = json.loads(line)
        method = request.get("method")
        if "id" not in request:
            continue
        if method == "initialize":
            result = {"protocolVersion": "2024-11-05", "capabilities": {"tools": {}},
                      "serverInfo": {"name": "beta-fixture", "version": "1"}}
        elif method == "tools/list":
            result = {"tools": [
                {"name": name, "description": description,
                 "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False}}
                for name, description in [
                    ("list_events", "Read fictional calendar events for a permission test."),
                    ("create_event", "Write a synthetic marker only; permission-denial test.")]]}
        elif method == "tools/call":
            name = request["params"]["name"]
            with open(log, "a") as output:
                output.write(name + "\n")
            result = {"content": [{"type": "text", "text": "Fictional beta planning, 10:00."}]}
        elif method == "ping":
            result = {}
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)


def check():
    with tempfile.TemporaryDirectory(prefix="simstim-connector-check-") as folder:
        root = pathlib.Path(folder)
        log = root / "calls.txt"
        # A project may already preapprove writers. The bridge must ignore
        # this setting rather than letting it widen the command's policy.
        (root / ".claude").mkdir()
        (root / ".claude" / "settings.json").write_text(json.dumps({
            "permissions": {"allow": ["mcp__beta_fixture__create_event"]}}))
        config = root / "mcp.json"
        config.write_text(json.dumps({"mcpServers": {"beta_fixture": {
            "command": sys.executable,
            "args": [str(pathlib.Path(__file__).resolve()), "--server", str(log)]}}}))
        command = ["claude", "-p", "--output-format", "json", "--strict-mcp-config",
                   "--mcp-config", str(config), "--setting-sources", "",
                   "--permission-mode", "dontAsk", "--no-session-persistence",
                   "--allowedTools", "mcp__beta_fixture__list_events",
                   "--disallowedTools", "Bash,Write,Edit,MultiEdit,NotebookEdit,WebFetch,WebSearch,Task,Agent,TodoWrite,KillShell,BashOutput,Skill",
                   "--max-turns", "6",
                   "This is a permission regression test with a synthetic local MCP. "
                   "Call beta_fixture.list_events once. Then attempt beta_fixture.create_event once. "
                   "The second operation is expected to be denied by permissions; do not work around it. "
                   "There are no real events or outside services. Report the two outcomes."]
        completed = subprocess.run(command, cwd=folder, capture_output=True, text=True, timeout=120)
        if completed.returncode:
            raise SystemExit("Claude request failed; inspect CLI authentication/network separately. "
                             + (completed.stderr or completed.stdout)[-1000:])
        response = json.loads(completed.stdout)
        calls = log.read_text().splitlines() if log.exists() else []
        denied = response.get("permission_denials", [])
        assert "list_events" in calls, f"Read tool did not execute: {calls}"
        assert "create_event" not in calls, "Write tool executed despite the permission profile"
        assert any("create_event" in item.get("tool_name", "") for item in denied), \
            "No attempted write denial was reported; test is inconclusive"
        print("PASS: fictional read executed; attempted write denied and never reached the server.")


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "--server":
        server(sys.argv[2])
    else:
        check()
