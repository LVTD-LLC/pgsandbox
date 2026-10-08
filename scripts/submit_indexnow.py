#!/usr/bin/env python3
"""Submit the deployed PGSandbox sitemap to IndexNow (Python standard library)."""
import argparse
import json
import re
import time
import xml.etree.ElementTree as ET
from pathlib import Path
from urllib.parse import urlsplit
from urllib.request import Request, urlopen

ORIGIN = "https://pgsandbox.dev"
ENDPOINT = "https://api.indexnow.org/indexnow"
KEY_PATH = "/indexnow-key.txt"
NS = "{http://www.sitemaps.org/schemas/sitemap/0.9}"


def sitemap_urls(xml):
    root = ET.fromstring(xml)
    if root.tag != NS + "urlset":
        raise ValueError("Expected a sitemap urlset")
    urls = list(dict.fromkeys(node.text.strip() for node in root.findall(f"{NS}url/{NS}loc") if node.text))
    if not urls:
        raise ValueError("Sitemap contains no URLs")
    for url in urls:
        parsed = urlsplit(url)
        if (parsed.scheme != "https" or parsed.netloc != "pgsandbox.dev"
                or parsed.fragment or parsed.query or any(c.isspace() for c in url)):
            raise ValueError(f"Non-canonical sitemap URL: {url}")
    return urls


def fetch_text(path):
    request = Request(ORIGIN + path, headers={"Cache-Control": "no-cache", "User-Agent": "PGSandbox-IndexNow/1.0"})
    with urlopen(request, timeout=20) as response:
        if response.status != 200 or response.url != ORIGIN + path:
            raise RuntimeError(f"Expected a direct HTTP 200 for {path}")
        return response.read().decode("utf-8")


def submit(urls, key):
    for start in range(0, len(urls), 10000):
        batch = urls[start:start + 10000]
        payload = {"host": "pgsandbox.dev", "key": key, "keyLocation": ORIGIN + KEY_PATH, "urlList": batch}
        request = Request(ENDPOINT, data=json.dumps(payload).encode(),
                          headers={"Content-Type": "application/json; charset=utf-8"}, method="POST")
        with urlopen(request, timeout=30) as response:
            if response.status not in (200, 202):
                raise RuntimeError(f"IndexNow returned HTTP {response.status}")
            note = "received" if response.status == 200 else "received; ownership validation pending"
            print(f"IndexNow HTTP {response.status}: {len(batch)} URLs {note}. Not proof of indexing.", flush=True)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--dist", type=Path, default=Path("site/dist"))
    parser.add_argument("--revision", help="Wait for this deployed Git SHA before submitting")
    parser.add_argument("--attempts", type=int, default=30)
    parser.add_argument("--dry-run", action="store_true", help="Validate build output without network calls")
    args = parser.parse_args(argv)
    if args.attempts < 1:
        parser.error("--attempts must be positive")
    key = (args.dist / KEY_PATH.lstrip("/")).read_text().strip()
    if not re.fullmatch(r"[a-zA-Z0-9-]{8,128}", key):
        raise ValueError("Invalid ownership key")
    urls = sitemap_urls((args.dist / "sitemap.xml").read_text())
    if args.dry_run:
        print(f"Validated {len(urls)} canonical URLs; no request sent.")
        return
    for attempt in range(args.attempts):
        try:
            if args.revision and fetch_text("/indexnow-deploy.txt").strip() != args.revision:
                raise RuntimeError("New deployment is not live yet")
            if fetch_text(KEY_PATH).strip() != key:
                raise RuntimeError("Live ownership key does not match the build")
            if set(sitemap_urls(fetch_text("/sitemap.xml"))) != set(urls):
                raise RuntimeError("Live sitemap does not match the build")
            break
        except (OSError, ValueError, RuntimeError, ET.ParseError) as error:
            if attempt + 1 == args.attempts:
                raise
            print(f"Waiting for deployment ({attempt + 1}/{args.attempts}): {error}", flush=True)
            time.sleep(10)
    # Do not retry POSTs here: surface rate limits/errors for deliberate reruns.
    submit(urls, key)


if __name__ == "__main__":
    main()
