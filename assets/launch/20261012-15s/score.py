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


# A new D-minor motif. Sparse phrasing leaves the code-reading and result holds quiet.
notes = [(0.05, 293.66, .62), (.82, 440, .44), (2.08, 349.23, .55),
         (2.8, 440, .43), (3.28, 523.25, .48), (4.13, 440, .52),
         (4.81, 659.25, .48), (6.58, 293.66, .52), (7.12, 349.23, .44),
         (7.62, 440, .55), (9.24, 440, .48), (9.81, 392, .46),
         (10.37, 349.23, .48), (11.28, 293.66, 1.08)]
for i, (t, frequency, duration) in enumerate(notes):
    score.place(pluck(frequency, duration), t, .065, (-.1 if i % 2 else .1), .28)

score.write(str(ROOT / "score.wav"), peak_db=-5.0)
(ROOT / "SOUND-CREDITS.md").write_text("""# Sound credits

The musical motif was composed for this A3S Code film. No existing recording,
commercial track, voice, or case-film mix was sampled. Action sounds use the
onetake synthetic palette: wood contacts, filtered air, one restrained pass
contact and a felt seal hit. All sounds share one short room and follow
events exported by the actual composition. The code-reading and final holds
leave deliberate silence. Palette code: see ONETAKE-LICENSE.txt.
""")
