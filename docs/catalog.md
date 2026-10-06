# Local Agent metadata and execution

`any-a2a catalog`, the desktop Agent list, and client capability hints read only the
local JSONL store. They do not fetch Agent Card URLs or check remote availability.
An unavailable remote Agent therefore cannot delay discovery of other saved Agents.

Adding a URL Agent explicitly fetches and validates its Card, then saves `cardUrl`
and the returned `agentCard` together. Subsequent discovery derives `info` from that
saved Card. These descriptions are last-known metadata and may have changed remotely.
Manual Agents derive their metadata from their locally supplied Card.

Older URL records that contain only `cardUrl` remain readable. Their catalog entries
have `raw: null` and `info: null`; clients display the saved ID and indicate that Card
metadata has not been cached. They must not invent a name, protocol, capability, or
scope. The desktop uses default request mode when the protocol is unknown.

Executing `run --agent-id ID` reads the selected local record. For a URL Agent it
fetches that Agent's current Card once and connects using the validated current
endpoint and saved authentication. It does not fetch unrelated Agents or silently
rewrite the saved metadata. Manual execution uses the selected saved Card directly.
Neither a catalog entry nor cached metadata proves a successful remote connection.

Catalog output omits the stored authentication fields. The JSONL file and its backups
still contain credentials and private metadata; do not share them. Existing legacy
`StoredAgent` import snapshots retain their format, while JSONL discovery uses the
nullable metadata fields in `CatalogAgent`.
