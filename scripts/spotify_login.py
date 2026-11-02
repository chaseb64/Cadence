"""One-time Spotify PKCE sign-in on the owner's PC. Standard library only."""
import argparse
import base64
import hashlib
import http.server
import json
import os
from pathlib import Path
import secrets
import time
import urllib.error
import urllib.parse
import urllib.request
import webbrowser

REDIRECT = "http://127.0.0.1:5588/callback"
SCOPES = "streaming user-read-playback-state user-modify-playback-state"


def challenge(verifier):
    return base64.urlsafe_b64encode(hashlib.sha256(verifier.encode("ascii")).digest()).rstrip(b"=").decode("ascii")


def validate_callback(path, expected_state, expected_path="/callback"):
    url = urllib.parse.urlsplit(path)
    params = urllib.parse.parse_qs(url.query)
    if url.path != expected_path:
        raise ValueError("Unexpected callback path")
    states = params.get("state", [])
    if len(states) != 1 or not secrets.compare_digest(states[0], expected_state):
        raise ValueError("Invalid OAuth state")
    if "error" in params:
        raise ValueError("Spotify authorization was declined")
    codes = params.get("code", [])
    if len(codes) != 1 or not codes[0]:
        raise ValueError("Missing authorization code")
    return codes[0]


def login(client_id, output, redirect=REDIRECT, scopes=SCOPES):
    if redirect not in (REDIRECT, "http://127.0.0.1:5588/login"):
        raise ValueError("Only the supported local loopback redirects are allowed")
    if output.exists():
        raise ValueError("Output already exists. Move it aside before repeating sign-in.")
    verifier, state = secrets.token_urlsafe(64), secrets.token_urlsafe(32)
    result = {}

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass  # Never log the callback URL or authorization code.

        def do_GET(self):
            try:
                result["code"] = validate_callback(self.path, state, urllib.parse.urlsplit(redirect).path)
                status, message = 200, b"Cadence sign-in received. You can close this tab."
            except ValueError:
                status, message = 400, b"Invalid sign-in callback. Return to the original sign-in tab."
            self.send_response(status)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(message)))
            self.end_headers()
            self.wfile.write(message)

    params = urllib.parse.urlencode({"client_id": client_id, "response_type": "code", "redirect_uri": redirect, "scope": scopes, "state": state, "code_challenge_method": "S256", "code_challenge": challenge(verifier)})
    with http.server.HTTPServer(("127.0.0.1", 5588), Handler) as server:
        server.timeout = 1
        url = "https://accounts.spotify.com/authorize?" + params
        print("Opening Spotify sign-in in your browser. This listener expires in five minutes.")
        if not webbrowser.open(url):
            print("Open this one-time sign-in URL on this PC:\n" + url)
        deadline = time.monotonic() + 300
        while "code" not in result:
            if time.monotonic() > deadline:
                raise ValueError("Sign-in timed out")
            server.handle_request()
    data = urllib.parse.urlencode({"grant_type": "authorization_code", "client_id": client_id, "code": result["code"], "redirect_uri": redirect, "code_verifier": verifier}).encode()
    request = urllib.request.Request("https://accounts.spotify.com/api/token", data=data)
    try:
        with urllib.request.urlopen(request, timeout=20) as response:
            token = json.load(response)
    except urllib.error.HTTPError as exc:
        raise ValueError(f"Spotify token exchange failed (HTTP {exc.code}); verify the app and redirect URI") from None
    refresh = token.get("refresh_token")
    if not refresh:
        raise ValueError("Spotify did not issue a refresh token")
    granted = set(token.get("scope", "").split())
    if not set(scopes.split()).issubset(granted):
        raise ValueError("Required Spotify permissions were not granted")
    output.parent.mkdir(parents=True, exist_ok=True)
    fd = os.open(output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w", encoding="utf-8") as stream:
        json.dump({"client_id": client_id, "refresh_token": refresh}, stream)
    print(f"Saved credential file to {output}. Keep it private; do not commit it.")
    print("Sign-in is complete. Spotify audio playback is still unverified.")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--client-id", required=True, help="Your Spotify developer app client ID (not a client secret)")
    parser.add_argument("--output", type=Path, default=Path("secrets/spotify.json"))
    parser.add_argument("--redirect-uri", default=REDIRECT)
    parser.add_argument("--scopes", default=SCOPES)
    args = parser.parse_args()
    try:
        login(args.client_id, args.output, args.redirect_uri, args.scopes)
    except (ValueError, OSError) as error:
        parser.exit(1, f"Sign-in failed: {error}\n")
