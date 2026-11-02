# NAS playback record

Status: NOT RUN. Do not mark Spotify supported until live evidence is recorded.

Local Windows validation completed: formatting and Clippy passed; all 11 Rust tests and 2 Python OAuth tests passed. Tests cover ownership, channel permissions, stale session state, bounded retries, request cancellation, buffer backpressure/flush/close, PCM format, track-link validation, and OAuth state/PKCE. Container build and live service tests remain pending.

| Item | Result |
| --- | --- |
| Host | UGreen DXP2800, Intel N100, linux/amd64 |
| Deployment | Portainer, standalone Docker assumed |
| UGOS / Docker version | Not recorded |
| Image tag / digest | Not built or deployed |
| Account | Owner reports Premium; no credentials collected in chat |
| Owner's app browser OAuth | Not run |
| Device visible in Spotify | Not run |
| Audible Discord playback / DAVE | Not run |
| Pause/resume/skip from both clients | Not run |
| Queue read/add synchronized | Not run |
| Transfer away/back | Not run |
| Restart / cached auth | Not run |
| Network / voice reconnect / SIGTERM | Not run |
| Voice-channel control restrictions | Not run |

Record dates, image digest, sanitized error categories, and human listening confirmation. Never paste credentials, authorization URLs, or tokens here.
