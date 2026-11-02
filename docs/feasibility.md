# NAS playback record

Status: NOT RUN. Do not mark Spotify supported until live evidence is recorded.

Local Windows validation completed: formatting and Clippy passed; all 14 Rust tests and 3 Python OAuth tests passed. Tests cover ownership, channel permissions, /join target selection, stale session state, bounded retries, request cancellation, buffer backpressure/flush/close, PCM format, track-link validation, and OAuth state/PKCE. Container build and live service tests remain pending.

| Item | Result |
| --- | --- |
| Host | UGreen DXP2800, Intel N100, linux/amd64 |
| Deployment | Portainer, standalone Docker assumed |
| UGOS / Docker version | UGOS 1.12.1.0002 / Docker 26.1.0 |
| Image tag / digest | sha-6ddc49d deployed; startup exposed Web API/Connect grant mismatch; corrected image pending |
| Account | Owner reports Premium; no credentials collected in chat |
| Owner's app browser OAuth | Completed; Web API refresh succeeded on NAS |
| Separate Connect browser grant | Completed on PC; full Connect session initialized with reusable credentials |
| Device visible in Spotify | Not run |
| Audible Discord playback / DAVE | Not run |
| Pause/resume/skip from both clients | Not run |
| Queue read/add synchronized | Not run |
| Transfer away/back | Not run |
| Restart / cached auth | Not run |
| Network / voice reconnect / SIGTERM | Not run |
| /join: initial join, channel move, missing voice, and control restrictions | Not run |

Record dates, image digest, sanitized error categories, and human listening confirmation. Never paste credentials, authorization URLs, or tokens here.
