#!/usr/bin/env python3
"""Print an access token for a LocalStack BugSplat API, for development and tests.

Runs the same OAuth flow `bugsplat login` uses (register a client, authorize with PKCE,
exchange the code), but posts the email and password to LocalStack's sign-in form
instead of opening a browser. LocalStack only: production has no such form.

    export BUGSPLAT_API_URL=http://localhost:8085
    export BUGSPLAT_ACCESS_TOKEN=$(scripts/localstack-token.py you@example.com 'password')
"""

import base64
import hashlib
import html
import http.cookiejar
import json
import re
import secrets
import sys
import urllib.error
import urllib.parse
import urllib.request

REDIRECT = "http://127.0.0.1:33418/callback"
LOCAL_HOSTS = ("localhost", "127.0.0.1", "::1")


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def main():
    if len(sys.argv) not in (3, 4):
        sys.exit("usage: localstack-token.py EMAIL PASSWORD [API_URL]")
    email, password = sys.argv[1], sys.argv[2]
    base = (sys.argv[3] if len(sys.argv) == 4 else "http://localhost:8085").rstrip("/")

    def require_local(url):
        parts = urllib.parse.urlparse(url)
        if parts.scheme not in ("http", "https") or parts.hostname not in LOCAL_HOSTS:
            sys.exit(f"refusing to send a request to {parts.scheme}://{parts.hostname}: not this machine")

    require_local(base)

    opener = urllib.request.build_opener(
        NoRedirect, urllib.request.HTTPCookieProcessor(http.cookiejar.CookieJar())
    )

    # Discovery documents and redirects name their own URLs, so check every one.
    def request(url, data=None, as_json=False):
        require_local(url)
        headers = {}
        if data is not None:
            if as_json:
                data, headers["Content-Type"] = json.dumps(data).encode(), "application/json"
            else:
                data = urllib.parse.urlencode(data).encode()
        try:
            response = opener.open(urllib.request.Request(url, data, headers))
        except urllib.error.HTTPError as error:
            response = error
        return response.status, response.headers, response.read().decode(), url

    def follow(result):
        status, headers, body, url = result
        while status in (301, 302, 303) and not headers["Location"].startswith(REDIRECT):
            status, headers, body, url = request(urllib.parse.urljoin(url, headers["Location"]))
        return status, headers, body, url

    def hidden_fields(page):
        pattern = r'type="hidden" name="(\w+)" value="([^"]*)"'
        return {name: html.unescape(value) for name, value in re.findall(pattern, page)}

    metadata = json.loads(request(f"{base}/.well-known/oauth-authorization-server")[2])
    client = json.loads(
        request(
            metadata["registration_endpoint"],
            {
                "client_name": "bugsplat-cli localstack-token",
                "redirect_uris": [REDIRECT],
                "grant_types": ["authorization_code", "refresh_token"],
                "response_types": ["code"],
                "token_endpoint_auth_method": "none",
            },
            as_json=True,
        )[2]
    )

    verifier = secrets.token_urlsafe(48)
    digest = hashlib.sha256(verifier.encode()).digest()
    challenge = base64.urlsafe_b64encode(digest).rstrip(b"=").decode()
    query = urllib.parse.urlencode(
        {
            "response_type": "code",
            "client_id": client["client_id"],
            "redirect_uri": REDIRECT,
            "code_challenge": challenge,
            "code_challenge_method": "S256",
            "state": secrets.token_urlsafe(16),
            "resource": f"{base}/mcp",
        }
    )

    status, headers, body, url = follow(request(f"{metadata['authorization_endpoint']}?{query}"))
    form = {**hidden_fields(body), "email": email, "password": password}
    status, headers, body, url = follow(request(url, form))
    if status == 200 and 'name="decision"' in body:
        status, headers, body, url = follow(
            request(url, {**hidden_fields(body), "decision": "allow"})
        )

    location = headers.get("Location") or ""
    code = urllib.parse.parse_qs(urllib.parse.urlparse(location).query).get("code", [None])[0]
    if not code:
        sys.exit(f"sign-in failed (HTTP {status}); check the email and password")

    token = json.loads(
        request(
            metadata["token_endpoint"],
            {
                "grant_type": "authorization_code",
                "code": code,
                "redirect_uri": REDIRECT,
                "client_id": client["client_id"],
                "code_verifier": verifier,
                "resource": f"{base}/mcp",
            },
        )[2]
    )
    if "access_token" not in token:
        sys.exit(f"token exchange failed: {token.get('error', 'unknown error')}")
    print(token["access_token"])


if __name__ == "__main__":
    main()
