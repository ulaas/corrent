"""A scratch media library of Blender Foundation open movies for the website's media screenshots.

    python -I scripts/site-media/library.py <empty scratch folder>

Downloads the films (480p, Sintel 720p), their posters and Caminandes' episode covers from Wikimedia
Commons (all Creative Commons, ~450 MB), writes Kodi .nfo files from the films' own release notes
(no ratings), takes one frame of each film as its backdrop (needs mpv on PATH or MPV=<mpv.com>), and
crops a 2:3 poster for Caminandes from an episode cover. Only free, openly licensed films: never put
anything else in screenshots. Existing files are kept, so a rerun only fetches what is missing.
Then run scripts/site-media/shots.ps1 with the same folder.
"""

import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from xml.sax.saxutils import escape

from PIL import Image

UA = {"User-Agent": "corrent-screenshots/1.0 (https://github.com/ulaas/corrent)"}
ROOT = Path(sys.argv[1])
MOVIES = ROOT / "Movies"
SHOW = ROOT / "Shows" / "Caminandes"
STUDIO = "Blender Foundation"

# folder, Commons video, wanted transcode, Commons poster, backdrop time, nfo fields
FILMS = [
    ("Sintel (2010)", "File:Sintel movie 4K.webm", "720p.vp9.webm", "File:Sintel poster.jpg", "0:05:40",
     dict(title="Sintel", year=2010, runtime=15, genre=["Animation", "Fantasy", "Adventure"], director="Colin Levy",
          tagline="A girl and her dragon.",
          plot="A lonely young woman, Sintel, helps and befriends a dragon she calls Scales. When he is taken by an "
               "adult dragon, Sintel sets out on a dangerous quest to find her lost friend.")),
    ("Big Buck Bunny (2008)", "File:Big Buck Bunny 4K.webm", "480p.vp9.webm", "File:Big buck bunny poster big.jpg", "0:01:15",
     dict(title="Big Buck Bunny", year=2008, runtime=10, genre=["Animation", "Comedy", "Family"], director="Sacha Goedegebure",
          tagline="The story of a giant rabbit with a heart bigger than himself.",
          plot="A large and lovable rabbit enjoys a peaceful morning in the forest until three rodent bullies, led by "
               "a flying squirrel, ruin his day. He plans a sweet revenge.")),
    ("Tears of Steel (2012)", "File:Tears of Steel 1080p.webm", "480p.vp9.webm", "File:Tos-poster.png", "0:03:20",
     dict(title="Tears of Steel", year=2012, runtime=12, genre=["Science Fiction", "Action"], director="Ian Hubert",
          tagline="A love story with robots.",
          plot="In a future Amsterdam, a group of warriors and scientists gathers at the Oude Kerk to stage a crucial "
               "event from the past, in a desperate attempt to rescue the world from destructive robots.")),
    ("Elephants Dream (2006)", "File:Elephants Dream (2006).webm", "480p.vp9.webm", "File:ElephantsDreamPoster.jpg", "0:02:30",
     dict(title="Elephants Dream", year=2006, runtime=11, genre=["Animation", "Science Fiction"], director="Bassam Kurdali",
          tagline="The world's first open movie.",
          plot="Two people, the old Proog and the young Emo, explore a strange and endless machine. Proog shows Emo "
               "its wonders, while Emo begins to doubt what the machine really is.")),
    ("Cosmos Laundromat (2015)", "File:Cosmos Laundromat - First Cycle - Official Blender Foundation release.webm",
     "480p.vp9.webm", "File:CosmosLaundromatPoster.jpg", "0:03:00",
     dict(title="Cosmos Laundromat", year=2015, runtime=12, genre=["Animation", "Fantasy", "Comedy"], director="Mathieu Auvray",
          tagline="First Cycle.",
          plot="On a desolate island, a suicidal sheep named Franck meets his fate in a quirky salesman, who offers "
               "him the gift of a lifetime: the chance to live another life.")),
]
EPISODES = [
    ("S01E01", "Llama Drama", "File:Caminandes- Llama Drama - Short Movie.ogv",
     "File:Pablo Vazquez - Caminandes - Episode 1 - Llama Drama - Cover thumbnail.png", "2013-01-17",
     "Koro the llama wants to cross a road, and the road has other ideas."),
    ("S01E02", "Gran Dillama", "File:Caminandes, Gran Dillama - Blender Foundation.webm",
     "File:Blender Foundation - Caminandes - Episode 2 - Gran Dillama - Cover thumbnail.png", "2013-11-22",
     "Koro wants the grass on the other side of an electric fence, and he will not give up."),
    ("S01E03", "Llamigos", "File:Caminandes 3 - Llamigos - Blender Animated Short.webm",
     "File:Blender Foundation - Caminandes - Episode 3 - Llamigos - Cover thumbnail.png", "2016-03-22",
     "In the winter, Koro meets Oti, a pesky penguin, and the two fight over the last berries in the valley."),
]


def api(**params):
    params["format"] = "json"
    url = "https://commons.wikimedia.org/w/api.php?" + urllib.parse.urlencode(params)
    return json.load(urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=60))


def page(title):
    r = api(action="query", titles=title, prop="videoinfo|imageinfo", viprop="url|derivatives", iiprop="url")
    return next(iter(r["query"]["pages"].values()))


def get(url, dest: Path):
    """Downloads with backoff: Commons answers 429 to quick runs of requests."""
    if dest.exists() and dest.stat().st_size > 0:
        return
    dest.parent.mkdir(parents=True, exist_ok=True)
    part = dest.with_name(dest.name + ".part")
    for attempt in range(8):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=UA), timeout=120) as r, open(part, "wb") as f:
                shutil.copyfileobj(r, f, 1 << 20)
            break
        except urllib.error.HTTPError as e:
            if e.code != 429 or attempt == 7:
                raise
            wait = int(e.headers.get("Retry-After") or 0) or 30 * (attempt + 1)
            print(f"429, waiting {wait} s", flush=True)
            time.sleep(wait)
    part.replace(dest)
    print(f"{dest.relative_to(ROOT)}: {dest.stat().st_size // 1048576} MB", flush=True)
    time.sleep(5)


def video(title, dest, want):
    if dest.exists():
        return
    p = page(title)
    d = {x["transcodekey"]: x["src"] for x in p["videoinfo"][0].get("derivatives", [])}
    get(d.get(want) or d.get("480p.vp9.webm") or p["videoinfo"][0]["url"], dest)


def image(title, dest):
    if dest.exists():
        return
    get(page(title)["imageinfo"][0]["url"], dest)


def nfo(path: Path, root: str, fields):
    body = "".join(f"  <{k}>{escape(str(v))}</{k}>\n" for k, vs in fields for v in (vs if isinstance(vs, list) else [vs]))
    path.write_text(f'<?xml version="1.0" encoding="UTF-8"?>\n<{root}>\n{body}</{root}>\n', encoding="utf-8")


def frame(src: Path, at: str, dest: Path):
    if dest.exists():
        return
    mpv = os.environ.get("MPV") or shutil.which("mpv.com") or shutil.which("mpv")
    if not mpv:
        print(f"no mpv: {dest.relative_to(ROOT)} skipped")
        return
    with tempfile.TemporaryDirectory() as tmp:
        subprocess.run([mpv, "--no-config", "--really-quiet", "--no-audio", f"--start={at}", "--frames=1",
                        "--vo=image", "--vo-image-format=jpg", "--vo-image-jpeg-quality=92",
                        f"--vo-image-outdir={tmp}", str(src)], check=False, capture_output=True)
        shots = list(Path(tmp).glob("*.jpg"))
        if shots:
            shutil.move(str(shots[0]), dest)


def main():
    for folder, vid, want, poster, at, m in FILMS:
        d = MOVIES / folder
        ext = Path(poster).suffix.lower()
        if not (d / "poster.jpg").exists():
            image(poster, d / f"poster{ext}")
            if ext != ".jpg":  # Tears of Steel's poster is a 14 MB PNG
                im = Image.open(d / f"poster{ext}").convert("RGB")
                im.thumbnail((1350, 2000))
                im.save(d / "poster.jpg", quality=92)
                (d / f"poster{ext}").unlink()
        video(vid, d / f"{folder}.webm", want)
        frame(d / f"{folder}.webm", at, d / "fanart.jpg")
        nfo(d / "movie.nfo", "movie", [("title", m["title"]), ("year", m["year"]), ("plot", m["plot"]),
                                       ("tagline", m["tagline"]), ("runtime", m["runtime"]), ("genre", m["genre"]),
                                       ("director", m["director"]), ("studio", STUDIO)])
    season = SHOW / "Season 01"
    for i, (se, title, vid, cover, aired, plot) in enumerate(EPISODES, 1):
        base = season / f"Caminandes - {se} - {title}"
        video(vid, base.with_suffix(".webm"), "480p.vp9.webm")
        image(cover, base.with_name(base.name + "-thumb.png"))
        nfo(base.with_suffix(".nfo"), "episodedetails", [("title", title), ("season", 1), ("episode", i), ("aired", aired),
                                                          ("plot", plot), ("director", "Pablo Vazquez")])
    nfo(SHOW / "tvshow.nfo", "tvshow", [
        ("title", "Caminandes"), ("year", 2013), ("premiered", "2013-01-17"),
        ("plot", "Koro the llama lives in Patagonia and keeps finding new ways into trouble, usually over a patch "
                 "of grass on the other side of a fence."),
        ("genre", ["Animation", "Comedy", "Family"]), ("director", "Pablo Vazquez"), ("studio", STUDIO)])
    frame(season / "Caminandes - S01E03 - Llamigos.webm", "0:01:40", SHOW / "fanart.jpg")
    if not (SHOW / "poster.jpg").exists():
        im = Image.open(season / "Caminandes - S01E02 - Gran Dillama-thumb.png").convert("RGB")
        w, h = im.size
        cw = h * 2 // 3
        x0 = max(0, min(w - cw, int(w * 0.585) - cw // 2))
        im.crop((x0, 0, x0 + cw, h)).resize((1000, 1500), Image.LANCZOS).save(SHOW / "poster.jpg", quality=92)
    print("library ready:", ROOT)


if __name__ == "__main__":
    main()
