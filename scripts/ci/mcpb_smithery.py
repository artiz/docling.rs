#!/usr/bin/env python3
"""Make the Smithery copy of an MCP bundle: the same .mcpb with each entry of
manifest.json's `tools` replaced by the server's own definition of that tool.

Smithery builds a server's card from the bundle's `manifest.tools`, and its
API requires every tool there to carry an `inputSchema` (`smithery mcp
publish` otherwise fails with `400 Invalid input: expected object, received
undefined`, once per tool). The MCPB schema `mcpb pack` validates against
allows only `name` and `description` on a tool, so the bundle on the release
stays as packed and only Smithery gets this copy. The definitions come from
the published server itself — the bundle's command run the way Smithery's
CLI runs it (`${user_config.*}` filled from the defaults, a scratch
directory for a required folder), then `tools/list` — so they cannot drift
from the code.

    mcpb_smithery.py BUNDLE OUT

Fails when the server lists other tools than the manifest names.
"""

import json
import os
import re
import subprocess
import sys
import tempfile
import zipfile

TIMEOUT = 600  # the first `uvx` run downloads the wheels


def hydrate(text, config, bundle_dir):
    def sub(m):
        key = m.group(1)
        if key == "__dirname":
            return bundle_dir
        if key.startswith("user_config.") and key[12:] in config:
            value = config[key[12:]]
            return str(value).lower() if isinstance(value, bool) else str(value)
        return m.group(0)

    return re.sub(r"\$\{([^}]+)\}", sub, text)


def server_tools(manifest, bundle_dir, scratch):
    config = {}
    for key, setting in manifest.get("user_config", {}).items():
        if "default" in setting:
            config[key] = setting["default"]
        elif setting.get("type") in ("directory", "file"):
            config[key] = scratch
    mc = manifest["server"]["mcp_config"]
    cmd = [hydrate(a, config, bundle_dir) for a in [mc["command"], *mc.get("args", [])]]
    env = dict(os.environ, **{k: hydrate(v, config, bundle_dir) for k, v in mc.get("env", {}).items()})
    print("running:", " ".join(cmd), flush=True)
    proc = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, env=env, text=True)
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "mcpb-smithery", "version": "0"}}},
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        {"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}},
    ]
    try:
        proc.stdin.write("".join(json.dumps(m) + "\n" for m in messages))
        proc.stdin.flush()
        for line in proc.stdout:
            message = json.loads(line)
            if message.get("id") == 2:
                if "error" in message:
                    sys.exit(f"tools/list failed: {message['error']}")
                return message["result"]["tools"]
        sys.exit(f"the server exited ({proc.wait()}) before listing its tools")
    finally:
        proc.kill()
        proc.wait()


def main(bundle, out):
    with tempfile.TemporaryDirectory() as tmp, zipfile.ZipFile(bundle) as zin:
        bundle_dir, scratch = os.path.join(tmp, "bundle"), os.path.join(tmp, "root")
        os.makedirs(scratch)
        zin.extractall(bundle_dir)
        manifest = json.loads(zin.read("manifest.json"))
        listed = {t["name"]: t for t in server_tools(manifest, bundle_dir, scratch)}
        named = [t["name"] for t in manifest.get("tools", [])]
        if set(named) != set(listed):
            sys.exit(f"the manifest names {sorted(named)}, the server lists {sorted(listed)}")
        manifest["tools"] = [listed[name] for name in named]
        with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zout:
            for item in zin.infolist():
                data = zin.read(item)
                if item.filename == "manifest.json":
                    data = json.dumps(manifest, indent=2, ensure_ascii=False).encode() + b"\n"
                zout.writestr(item, data)
    print(f"{out}: {len(named)} tools with their input schemas")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit(__doc__)
    main(sys.argv[1], sys.argv[2])
