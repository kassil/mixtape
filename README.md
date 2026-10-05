# mixtape

A terminal mixtape player and loudness normalizer for Side A / Side B audio files.

## FFmpeg

The `N` normalize scan uses the `ffmpeg` command-line program. Install FFmpeg and make sure it is available in your `PATH` before scanning audio files.

Check the installation with:

```sh
ffmpeg -version
```

If FFmpeg cannot be started, mixtape shows the affected filenames and the reason in the scan review modal.
