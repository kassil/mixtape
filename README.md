# mixtape

A terminal mixtape player and loudness normalizer for Side A / Side B audio files.

## FFmpeg

The `N` normalize scan uses the `ffmpeg` command-line program. Install FFmpeg and make sure it is available in your `PATH` before scanning audio files.

Check the installation with:

```sh
ffmpeg -version
```

If FFmpeg cannot be started, mixtape shows the affected filenames and the reason in the scan review modal.

## Track durations

Track lengths are loaded in the background when a directory is opened. The main view shows each track length and the total time for Side A and Side B. Totals include the pause between adjacent tracks.

Use `[` and `]` to decrease or increase the pause by one second. The default pause is two seconds.
