"""Freeze the composition's events and check seek purity before shutter rendering."""
import asyncio
import hashlib
import json
from pathlib import Path

from playwright.async_api import async_playwright

ROOT = Path(__file__).resolve().parent


async def main():
    errors = []
    async with async_playwright() as pw:
        browser = await pw.chromium.launch(headless=True)
        page = await browser.new_page(viewport={"width": 1920, "height": 1080})
        page.on("pageerror", lambda e: errors.append(str(e)))
        await page.goto((ROOT / "film.html").as_uri())
        await page.evaluate("window.__ready")
        for frame in range(1801):
            await page.evaluate("t => window.__seek(t)", frame / 30)
        hashes = []
        for t in (24.0, 5.0, 24.0):
            await page.evaluate("t => window.__seek(t)", t)
            hashes.append(hashlib.sha256(await page.screenshot()).hexdigest())
        assert hashes[0] == hashes[2], "A repeated seek must reproduce identical pixels"
        await page.evaluate("window.__seek(58.0)")
        await page.screenshot(path=str(ROOT / "poster.png"))
        events = await page.evaluate("window.__events()")
        (ROOT / "events.json").write_text(json.dumps(events, indent=2) + "\n")
        await browser.close()
    assert not errors, errors
    report = {"sampled_frames": 1801, "seek_errors": errors, "repeated_seek_pixels_identical": True, "pixel_hashes": hashes}
    (ROOT / "composition-check.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report))


if __name__ == "__main__":
    asyncio.run(main())
