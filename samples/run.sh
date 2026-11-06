#!/usr/bin/env sh
# Run every sample through notebender. Needs ANTHROPIC_API_KEY in the environment.
# Usage: samples/run.sh [extra notebender flags, e.g. --review or --model omr=fable]
set -eu
cd "$(dirname "$0")/.."
NB="${NOTEBENDER:-cargo run --release -q --}"
FMT="--format musicxml,lilypond,pdf"

$NB tab "samples/Tarrega - Gran Valse (orig).pdf" -o out/tarrega $FMT "$@"
$NB tab "samples/Sibelius - Etude Op. 76, No. 2 (cello and guitar).pdf" -o out/sibelius $FMT "$@"
$NB run "samples/Bach - Andante from Gamba Sonata (Violoncello) in G major BWV 1027-piano score.pdf" \
  -p "Arrange for cello and guitar, keeping the cello line as written and giving the piano part to the guitar. Add tablature for the guitar." \
  -o out/bach "$@"
$NB run "samples/Granados - intermedio-from-goyescas (flute, guitar).pdf" \
  -p "Rearrange for cello and guitar: the flute line goes to the cello (adjust octaves and idioms as needed). Add tablature for the guitar." \
  -o out/granados "$@"
$NB transpose "samples/Jun-Dai - Notebook - 1. Stroll.pdf" -s 1 -o out/stroll $FMT "$@"
$NB transpose "samples/Jun-Dai - Prelude.pdf" -s 7 -o out/prelude $FMT "$@"
