"""Medians of measure.sh's results.txt, one line per tool and file."""
import collections
import statistics
import sys

runs = collections.defaultdict(list)
for line in open(sys.argv[1]):
    tool, file, _run, title, frame, rss = line.split()
    runs[(tool, file)].append((title, frame, rss))

for (tool, file), rows in runs.items():
    def med(i):
        vals = [int(r[i]) for r in rows if r[i] != "NA"]
        return statistics.median(vals) if vals else "-"
    print(f"{tool:5} {file:9} runs {len(rows)}  window {med(0)} ms  first frame {med(1)} ms  rss {med(2)} MB")
