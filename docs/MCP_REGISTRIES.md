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
| [Smithery](https://smithery.ai) | the MCP bundle `docling-rs-mcp-<version>.mcpb`, packed from [`crates/docling-py/mcp/mcpb/`](../crates/docling-py/mcp/mcpb/manifest.json): `uvx docling-rs-mcp@<version>` with the folder, URL fetching and OCR language as settings; [`smithery.yaml`](../smithery.yaml) only names the server id | the Smithery API key of the namespace it is published under |
| [Glama](https://glama.ai/mcp/servers) | the repository itself; [`glama.json`](../glama.json) names the maintainers | a maintainer listed there claims the entry |

Smithery publishes a server either as an HTTPS URL (a hosted endpoint —
the *Publish an MCP Server* form on smithery.ai asks only for that) or as
an MCP bundle (`.mcpb`, a zip of a manifest and an icon that Smithery's
clients run locally), which `smithery mcp publish` uploads. This server is
local, so it is the bundle; the old `smithery.yaml` keys (`startCommand`,
`commandFunction`) are no longer read. A server id is 3–39 letters, digits,
`-` or `_`, starting with a letter: `docling-rs`, not `docling.rs`.

PyPI has no macOS wheel of `docling-rs` (no hosted macOS runners in the
release workflow), so on a Mac the uvx routes, the bundle included,
compile the package on first start and need a Rust toolchain; run
`uvx docling-rs-mcp --download-models` in a terminal once before adding
the server to a client.

`crates/docling-cli/tests/mcp_registry.rs` keeps these files consistent: the
same name everywhere, the registry's 100-character caps, the ownership
markers in place, every flag the catalogs pass one the server takes
(checked against the binary's `--help`), and the bundle as Smithery's CLI
reads it — a server id it accepts, the `python` runtime type (it rejects
MCPB's `uv`), the launcher pinned to the bundle's version, every
`${user_config.*}` declared (optional ones with a default, since Smithery
substitutes only values it has), and the tools the server lists.

## On a PyPI publish

PyPI publishing is manual (`gh workflow run pypi-publish.yml -f tag=vX.Y.Z`,
crates/docling-py/README.md — a release per merge outgrew PyPI's project size
limit), and the catalog entries follow it:

1. `docker-publish.yml` has published the images on the GitHub Release; the
   CLI image carries the registry label.
2. `pypi-publish.yml` publishes the `docling-rs` wheels, then the
   `docling-rs-mcp` launcher pinned to that version (a separate job, so a
   launcher problem never holds back `docling-rs`).
3. For a release tag it then calls `mcp-registry.yml`, which waits until
   `docling-rs-mcp==<version>` is on PyPI and
   `ghcr.io/docling-project/docling-rs:<version>` on GHCR, writes the version
   into `server.json` (from the release tag), and publishes it with
   `mcp-publisher` — authenticated by GitHub OIDC, no secret. Its `bundle`
   job waits for the launcher alone, packs `docling-rs-mcp-<version>.mcpb`
   (`mcpb pack` validates the manifest) and attaches it to the release; when
   the repository has the `SMITHERY_API_KEY` secret and the `SMITHERY_SERVER`
   variable it publishes Smithery's copy of it, whose manifest carries each
   tool's input schema as the server lists it (`scripts/ci/mcpb_smithery.py`
   — Smithery's API requires one per tool, the MCPB schema allows none).
   Both jobs run only in `docling-project/docling.rs`.

## One-time setup

1. **PyPI** — the launcher is a new project, so PyPI must trust this
   workflow before its first upload: pypi.org → *Your account* →
   *Publishing* → *Add a new pending publisher*: project `docling-rs-mcp`,
   owner `docling-project`, repository `docling.rs`, workflow
   `pypi-publish.yml`, environment `pypi`. Until then
   `publish-mcp-launcher` fails (nothing else does) and the registry step
   never runs; re-run the failed jobs once it is set.
2. **Official registry** — nothing: the OIDC token from this repository
   proves the `io.github.docling-project` namespace (the org already
   publishes `io.github.docling-project/docling-mcp` that way).
3. **Smithery** — create an API key on smithery.ai (account settings → API
   keys) and add it to `docling-project/docling.rs` as the Actions secret
   `SMITHERY_API_KEY`, plus the Actions variable `SMITHERY_SERVER` =
   `<namespace>/docling-rs` (the namespace is the Smithery user or
   organization that owns the listing, e.g. `artem-kustikov/docling-rs`).
   The first PyPI publish after that creates the server; until both are set
   the `bundle` job only packs and attaches the bundle. Skip the web form: it
   publishes hosted URLs only.
4. **Glama** — on glama.ai, *Add MCP Server* → *Runs from source*: name
   `docling-rs-mcp`, a one- or two-sentence description, repository
   `https://github.com/docling-project/docling.rs`. After review Glama mails
   how to provide the Dockerfile its checks build. The server needs no
   models to start and list its tools, so the lightest one is a Python
   image with uv that runs `uvx docling-rs-mcp`; the CLI image
   (`crates/docling-serve/Dockerfile`, target `cli`, `mcp --root /data`)
   works too. A maintainer from `glama.json` claims the listing; add
   maintainers there before they claim.

## Publishing by hand

When a publish needs a re-run (`mcp-registry.yml` has a `version` input on
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

The Smithery bundle for a version already on PyPI (the release carries it as
`docling-rs-mcp-<version>.mcpb`; to pack it yourself):

```bash
v=2.22.0
jq --arg v "$v" '.version = $v | .server.mcp_config.args |= map(if startswith("docling-rs-mcp@") then "docling-rs-mcp@" + $v else . end)' crates/docling-py/mcp/mcpb/manifest.json > /tmp/manifest.json
cp /tmp/manifest.json crates/docling-py/mcp/mcpb/manifest.json
npx -y @anthropic-ai/mcpb@2.1.2 pack crates/docling-py/mcp/mcpb "docling-rs-mcp-$v.mcpb"
git checkout crates/docling-py/mcp/mcpb/manifest.json
# Smithery's copy: the tools' input schemas from the server (runs it with uvx)
python3 scripts/ci/mcpb_smithery.py "docling-rs-mcp-$v.mcpb" "smithery-$v.mcpb"
npx -y @smithery/cli@4.11.1 auth login   # or export SMITHERY_API_KEY
npx -y @smithery/cli@4.11.1 mcp publish "smithery-$v.mcpb" -n <namespace>/docling-rs
```

Publishing the packed bundle itself fails with `400 Invalid input: expected
object, received undefined` — once per tool, for the missing input schemas.

## Checking a release

```bash
curl -s "https://registry.modelcontextprotocol.io/v0/servers?search=io.github.docling-project/docling-rs" | jq '.servers[].server | {name, version}'
uvx docling-rs-mcp --help
npx @modelcontextprotocol/inspector --cli uvx docling-rs-mcp --root ~/Documents --method tools/list
npx -y @smithery/cli@4.11.1 mcp search docling-rs
```
