# Sample scores

Real-world inputs for trying `notebender` end to end. They are not used by the
test suite (tests stay offline). Every run below needs `ANTHROPIC_API_KEY`
and the tools listed by `notebender doctor`; `samples/run.sh` runs them all.

| File | What it is | Intended request |
| --- | --- | --- |
| `Tarrega - Gran Valse (orig).pdf` | Solo guitar, standard notation only, 3 pages (A3 engraving) | Add tablature |
| `Sibelius - Etude Op. 76, No. 2 (cello and guitar).pdf` | Arrangement for cello and guitar, 4 pages | Add tablature to the guitar part |
| `Bach - Andante from Gamba Sonata ... BWV 1027-piano score.pdf` | Piano + cello, 2 pages | Arrange for cello and guitar, with tablature |
| `Granados - intermedio-from-goyescas (flute, guitar).pdf` | Flute + guitar, 4 pages (Sibelius engraving) | Rearrange for cello and guitar, with tablature |
| `Jun-Dai - Notebook - 1. Stroll.pdf` | Handwritten piano piece, 6/8, 2 pages | Transpose up a half step (tests handwriting OMR) |
| `Jun-Dai - Prelude.pdf` | Handwritten piano piece, 7/8, D-flat major, 5 pages | Transpose up a fifth (tests handwriting OMR) |

Each job writes `score.musicxml`, `score.ly`, `score.pdf` and a `report.md`
with per-step model usage and cost under `out/<name>/`.
