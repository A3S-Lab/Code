"""Original restrained motif and tactile action score; event times come from film.html."""
import json
from pathlib import Path

import numpy as np
from scipy import signal

from sfx_palette import SR, Score, air, glass, wood, sub

ROOT = Path(__file__).resolve().parent
data = json.loads((ROOT / "events.json").read_text())
score = Score(dur=data["T"], T60=0.65)

for event in data["events"]:
    t, kind, pan, v = (event[k] for k in ("t", "kind", "pan", "v"))
    if kind == "key":
        score.place(wood(215, .075), t, .20 * v, pan, .12)
    elif kind == "open":
        score.place(air(.55, 280, 2100, 1.3, .4), t, .34 * v, pan, .32, -.12)
    elif kind == "land":
        score.place(wood(155, .15), t, .45 * v, pan, .18)
    elif kind == "focus":
        score.place(air(.30, 550, 2200, 1.5, .35), t, .32 * v, pan, .25, -.4)
    elif kind == "edit":
        score.place(wood(285, .11), t, .45 * v, pan, .14)
    elif kind == "run":
        score.place(wood(250, .10), t, .42 * v, pan, .18)
    elif kind == "pass":
        score.place(wood(320, .10) + glass(1174, .10, .16) * .18, t, .40 * v, pan, .22)
    elif kind == "fold":
        score.place(air(.7, 2100, 200, 1.25, .4), t, .40 * v, pan, .36, -.35)
    elif kind == "seal":
        score.place(sub(78, .48), t, .35 * v, pan, .24)
        score.place(wood(168, .12), t, .35 * v, pan, .18)


def pluck(frequency, duration):
    t = np.arange(int(SR * duration)) / SR
    partials = (np.sin(2 * np.pi * frequency * t)
                + .32 * np.sin(2 * np.pi * frequency * 2.01 * t)
                + .13 * np.sin(2 * np.pi * frequency * 3.99 * t))
    envelope = (1 - np.exp(-t * 150)) * np.exp(-t * 7)
    fade = np.minimum(1, np.maximum(0, duration - t) / .07)
    return signal.sosfilt(signal.butter(2, 4200, fs=SR, output="sos"), partials * envelope * fade)


# Original D-minor phrases with room for actual code-reading and menu holds.
# These are authored for 60 seconds, rather than four repetitions of the short film.
notes = [
 (0.1,293.66,.9),(.95,440,.6),(2.4,349.23,.75),(3.45,440,.6),
 (4.12,523.25,.75),(6.25,440,.7),(7.1,349.23,.7),(9.0,293.66,1.1),
 (10.65,440,.8),(12.4,659.25,.7),(13.8,523.25,1.0),
 (18.85,293.66,.8),(20.1,349.23,.7),(22.75,440,.8),(23.8,523.25,.9),
 (28.1,440,.7),(28.8,392,.6),(30.25,349.23,.7),(31.65,293.66,1.0),
 (33.1,349.23,.7),(36.7,440,.6),(37.3,523.25,.6),(38.0,659.25,.65),
 (40.85,523.25,.75),(41.65,440,.8),(43.6,349.23,.7),
 (45.6,440,.65),(46.85,523.25,.8),(48.35,440,.8),
 (51.45,392,.6),(52.15,349.23,.7),(53.3,440,.6),(54.1,293.66,1.3)
]
for i, (t, frequency, duration) in enumerate(notes):
    score.place(pluck(frequency, duration), t, .062, (-.1 if i % 2 else .1), .28)

# Soft harmonic beds appear only beneath the short active phrases.
def bed(frequency, duration):
    tt = np.arange(int(SR * duration)) / SR
    env = np.minimum(tt / 1.0, 1) * np.minimum((duration-tt)/1.2, 1)
    tone = np.sin(2*np.pi*frequency*tt) + .22*np.sin(2*np.pi*frequency*2*tt)
    return tone * env * .14
for t, frequency, duration in [(0,146.83,3.1),(3.4,146.83,6.0),(28.1,130.81,4.4),(37.2,146.83,4.0),(46.6,174.61,3.3),(52.1,146.83,3.3)]:
    score.place(bed(frequency,duration),t,.075,0,.3)

score.write(str(ROOT / "score.wav"), peak_db=-5.0)
(ROOT / "SOUND-CREDITS.md").write_text("""# Sound credits

The 60-second musical score was composed for this A3S Code film. No existing recording,
commercial track, voice, or case-film mix was sampled. Action sounds use the
onetake synthetic palette: wood contacts, filtered air, one restrained pass
contact and a felt seal hit. All sounds share one short room and follow
events exported by the actual composition. The code-reading and final holds
leave deliberate silence. Palette code: see ONETAKE-LICENSE.txt.
""")
