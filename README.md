# mixtape

A terminal mixtape player and Side A / Side B preparation tool.

## FFmpeg

Side Prep uses the `ffmpeg` command-line program. Install FFmpeg and make sure it is available in your `PATH` before scanning or processing tracks.

Check the installation with:

```sh
ffmpeg -version
```

Select a Side A or Side B track and press `N` to scan from that track through the end of its side. The Side Prep report includes every scanned track, its peak level, and leading/trailing silence measurements. Silence is measured below -48 dB; runs shorter than 0.1 seconds are ignored.

Side Prep recommends trimming an edge when its measured silence is at least one second. Processing keeps about 0.9 seconds at that edge. It recommends peak normalization when the peak is outside -1.5 to -0.5 dB and targets -1 dB. Tracks with both edge silences below one second and a peak within that range are reported but are not re-encoded.

Use Up/Down to select a report row and Enter to apply that track's recommended edits. Trimming and normalization are combined in one FFmpeg pass. Processing replaces the original file only after FFmpeg successfully writes the temporary output; OGG and MP3 tracks are re-encoded. Esc cancels scanning or processing, or closes the report. If FFmpeg cannot be started or a track cannot be analyzed, Side Prep shows the affected filename and error.

## Track durations

Track lengths are loaded in the background when a directory is opened. The main view shows each track length and the total time for Side A and Side B. Totals include the pause between adjacent tracks.

Use `[` and `]` to decrease or increase the pause by one second. The default pause is two seconds. The main view shows `[N] Side Prep` as the scan control.
