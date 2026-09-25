# Themes

Bjorn reads theme files in Bear's `.theme` format from
`~/.config/bjorn/themes/` (or `$XDG_CONFIG_HOME/bjorn/themes/`, or `themes/`
beside the file `--config` names); see
[Your own themes](../../README.md#your-own-themes) in the main README.

## Bear's themes

Bear's own theme files are not in this repository. They are the work of
[Shiny Frog](https://bear.app), Bear's developer, and are not ours to
redistribute. Every one of them is built into Bjorn already, as a palette
generated from the file by `tools/bear_theme.py` (`bjorn --list-themes`).

To start your own theme from one of Bear's, copy it out of your Bear.app:

```sh
mkdir -p ~/.config/bjorn/themes
cp "/Applications/Bear.app/Contents/Frameworks/BearCore.framework/Versions/A/Resources/Academia.theme" \
   ~/.config/bjorn/themes/"My Academia.theme"
```

Give it a new file name: a built-in name always wins over a file, so a copy
called `Academia.theme` is never used, and `--list-themes` says so. `My
Academia.theme` is `my-academia`. A theme whose `meta."base theme"` names one
of Bear's themes (Academia's is Dark Graphite) needs nothing else copied: a
base that is not in the directory comes from the built-in theme of that name.

[`bear-themes.sha256`](bear-themes.sha256) lists the name and SHA-256 of each
of Bear's theme files that `palettes.rs` was generated from, and nothing of
their contents. Where Bear.app is installed, the test
`bear_theme_files_match_the_built_in_palettes` reads Bear's files from the
bundle, checks them against this list, and checks that each one the list
matches parses to exactly its built-in palette. A file whose hash differs is
noted and left out, never failed: it means Bear updated its themes, and
`tools/bear_theme.py` is due a rerun. Loading a theme never checks a hash;
any file in your themes directory is used whatever it contains. To compare
your Bear.app against the list by hand:

```sh
cd /Applications/Bear.app/Contents/Frameworks/BearCore.framework/Versions/A/Resources
shasum -a 256 -c ~/path/to/bjorn/config/themes/bear-themes.sha256
```

## Bjorn's themes

Written for Bjorn, and covered by this project's license. They are starting
points for your own and the fixtures for `hand_written_theme_files_load`,
which runs everywhere, Bear or not:

| File | |
|---|---|
| `Paper.theme` | a light theme with every key Bjorn reads, most of them `$section.key` references |
| `Paper Night.theme` | a dark theme over `Paper` as its base theme, overriding only `base` |
| `Nord Ember.theme` | the built-in Nord with an orange accent: a base theme that is not a file |
