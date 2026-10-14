# Listing the MCP server in MCP catalogs

The docling.rs MCP server (#669) is one server with three ways to run it,
and the catalogs list all of them under one name,
**`io.github.docling-project/docling-rs`**:

| How | Command | PDF models |
|---|---|---|
| PyPI launcher | `uvx docling-rs-mcp --root ~/Documents` | `uvx docling-rs-mcp --download-models` once (into `~/.cache/docling.rs`) |
| The binary | `docling-rs mcp --root ~/Documents` | `download_dependencies.sh --cli` once |
| The CLI image | `docker run -i --rm -v ~/Documents:/data ghcr.io/docling-project/docling-rs mcp --root /data` | baked in |

The launcher (`crates/docling-py/mcp`, PyPI `docling-rs-mcp`) has no server
code of its own: it pins the matching `docling-rs` wheel, which links the
same Rust server the binary runs (`docling_rs.mcp`, built from
`crates/docling-cli/src/mcp.rs`). Every catalog therefore runs exactly what
`docling-rs mcp` runs.

## What each catalog reads

| Catalog | Reads | Ownership check |
|---|---|---|
| [Official MCP registry](https://registry.modelcontextprotocol.io) | [`server.json`](../server.json) (schema 2025-12-11): the PyPI launcher (`uvx`) and the CLI image (OCI) | `mcp-name: io.github.docling-project/docling-rs` in the launcher's README (its PyPI description); `LABEL io.modelcontextprotocol.server.name` on the image (`crates/docling-serve/Dockerfile`); the namespace by GitHub OIDC from this repository |
| [Smithery](https://smithery.ai) | [`smithery.yaml`](../smithery.yaml): a local stdio server started with `uvx docling-rs-mcp --root …`, the root and URL fetching as settings | the GitHub repository connected to the Smithery account |
| [Glama](https://glama.ai/mcp/servers) | the repository itself; [`glama.json`](../glama.json) names the maintainers | a maintainer listed there claims the entry |

`crates/docling-cli/tests/mcp_registry.rs` keeps these files consistent: the
same name everywhere, the registry's 100-character caps, the ownership
markers in place, and every flag the catalogs pass one the server takes
(checked against the binary's `--help`).

## On every release (automatic)

1. `pypi-publish.yml` publishes the `docling-rs` wheels, then the
   `docling-rs-mcp` launcher pinned to that version (a separate job, so a
   launcher problem never holds back `docling-rs`).
2. `docker-publish.yml` publishes the images; the CLI image carries the
   registry label.
3. `mcp-registry.yml` waits until `docling-rs-mcp==<version>` is on PyPI and
   `ghcr.io/docling-project/docling-rs:<version>` on GHCR, writes the version
   into `server.json`, and publishes it with `mcp-publisher` — authenticated
   by GitHub OIDC, no secret. It runs only in `docling-project/docling.rs`.

## One-time setup

1. **PyPI** — the launcher is a new project, so PyPI must trust this
   workflow before its first upload: pypi.org → *Your account* →
   *Publishing* → *Add a new pending publisher*: project `docling-rs-mcp`,
   owner `docling-project`, repository `docling.rs`, workflow
   `pypi-publish.yml`, environment `pypi`. Until then
   `publish-mcp-launcher` fails (nothing else does) and `mcp-registry.yml`
   times out waiting for it; re-run both once it is set.
2. **Official registry** — nothing: the OIDC token from this repository
   proves the `io.github.docling-project` namespace (the org already
   publishes `io.github.docling-project/docling-mcp` that way).
3. **Smithery** — sign in at smithery.ai with GitHub, add a server from the
   `docling-project/docling.rs` repository (Smithery's GitHub app needs
   access to it, which an org admin grants) and let it read `smithery.yaml`.
4. **Glama** — the public repository is indexed on its own; a maintainer
   from `glama.json` claims the listing on glama.ai. Add maintainers there
   before they claim.

## Publishing by hand

When a release needs a re-run (`mcp-registry.yml` has a `version` input on
*Run workflow*), or to publish from a laptop as a `docling-project` member:

```bash
curl -fsSL "https://github.com/modelcontextprotocol/registry/releases/latest/download/mcp-publisher_$(uname -s | tr '[:upper:]' '[:lower:]')_$(uname -m | sed 's/x86_64/amd64/;s/aarch64/arm64/').tar.gz" | tar xz mcp-publisher
v=2.20.0   # a version already on PyPI and GHCR
jq --arg v "$v" '.version = $v | .packages |= map(if .registryType == "oci" then .identifier |= sub(":[^:/]*$"; ":" + $v) else .version = $v end)' server.json > /tmp/server.json
cp /tmp/server.json server.json
./mcp-publisher login github     # device flow in the browser
./mcp-publisher publish
git checkout server.json
```

## Checking a release

```bash
curl -s "https://registry.modelcontextprotocol.io/v0/servers?search=io.github.docling-project/docling-rs" | jq '.servers[].server | {name, version}'
uvx docling-rs-mcp --help
npx @modelcontextprotocol/inspector --cli uvx docling-rs-mcp --root ~/Documents --method tools/list
```
