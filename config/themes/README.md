# Themes

Theme files in Bear's `.theme` format. Bjorn reads any of them from
`~/.config/bjorn/themes/` (or `$XDG_CONFIG_HOME/bjorn/themes/`); see
[Your own themes](../../README.md#your-own-themes) in the main README.

## Bear's themes

The 39 files below are Bear's own themes, copied unchanged from Bear 2.10
(build 14800) at
`Bear.app/Contents/Frameworks/BearCore.framework/Versions/A/Resources/`. They are
the work of [Shiny Frog](https://bear.app), Bear's developer, and remain
theirs; they are not covered by this project's license. They are here as
starting points for your own themes, and as the fixtures for the test
`repo_theme_files_match_the_built_in_palettes`, which checks that the parser
reads each one exactly as `tools/bear_theme.py` generated its built-in
palette.

Every one of them is also built in under the same name, and a built-in name
wins over a file, so to change one, copy it into `~/.config/bjorn/themes/`
under a new file name (`My Nord.theme` is `my-nord`) and edit it.

The SHA-256 of each file as copied, so a changed or replaced file shows up:

| File | SHA-256 |
|---|---|
| `Academia.theme` | `8b9113ddc6df390f42d2903c60a9a850eb11195af74dcbd0eade36b5d6efc940` |
| `Atom.theme` | `e97b234e98bdfa7d047d6a4edac177139568a22d259f1da08677ed7e871375c3` |
| `Ayu Mirage.theme` | `45990553007848c72eac92bba20f55fb50ca72eecca0389fb60b33b3bee15edc` |
| `Ayu.theme` | `e9af7d3be95ca5e33a7ef1123a1ab430563187a560fbb42bb98f5d9b3fb7a1ef` |
| `Catppuccin Latte.theme` | `494ba6fc7d231c9ada9203a428c63f31dc2e6c4f1871309395ed4bbb89503be6` |
| `Catppuccin Macchiato.theme` | `8d6d9855740e8482669e802da6eb004b57381f43ba5d0b3f4ad9c930c6d4e841` |
| `Charcoal.theme` | `c7e94ee41a51317325221a484800ee19a157f530db123ec839bea61c5bc57063` |
| `Cobalt.theme` | `dd81e7866f135937e1868cb6034982d024351d0ab692dede1fa2d262e2afe641` |
| `D.Boring.theme` | `4223547d63af8f71fb51ed555733674c7b21a97c44d1be29e5f595a724f12247` |
| `Dark Graphite.theme` | `1efc36d0c381f3eecedd67b8534848ae39a7a36c71380f44916fe5448f0bc36f` |
| `Dark Notes.theme` | `d9a7f4bf8fc8a8132e7d63f1ee32a3c767f5550039c641cd3c0f14f0dbc7944a` |
| `Dieci.theme` | `b3892811be2ea77622ce2227b845bf7da159c7693d11f56f8c7b842579ffe78b` |
| `Dracula.theme` | `2f6aa6e2cc2071753e086dddd4f6b5511fd6aac9c5fdc767ec64746a669c618c` |
| `Duotone Heat.theme` | `5b9e153015dd91f53e4b2bc9e085aa9fdc67e6e48bd40f6ea69d9fd823d70f23` |
| `Duotone Light.theme` | `cadb2860d379951e04d3deae6b2945c066c87be24dca1ed4095bcb5f08c32b99` |
| `Duotone Snow.theme` | `8b333355edc57f989858eb41219a33a08209f2dd8c8c6be2fb6e886e1d3c9b74` |
| `Everforest Dark.theme` | `207a59c8529156a088136de326121bc2462b7e6cb04116b67d2b111b6add3ffd` |
| `Everforest Light.theme` | `db53722129a8f4be7b7a393d9fd4ac2dd6f93512823b686faf5a3df325ab5569` |
| `Gandalf.theme` | `52e122885eb5d8bf35eb7c9ef94c5148eb835a6e89ccbd2310ccc6f16cb80f08` |
| `Gotham.theme` | `ac0dea5e4fd4b70bb9d937e8d6cb5310a7c427ea6fb7762022f1f69b10c75da4` |
| `Gruvbox.theme` | `91c96ee0d92a139a241b567a5aab1f1fcf41447def99db707e1aef6bffec24bd` |
| `High Contrast.theme` | `4f30067030ad18b154e686a416cc1d568995d85921f1f96345e549731bddd871` |
| `Lighthaus.theme` | `e580f9302d754754e5a43b8179e50febca8a01efc2b328aa361206b6fc78a9df` |
| `Nord Light.theme` | `33023b5230488896655968b30f9620bb1d1489231422e72f42fbc7366c65fc28` |
| `Nord.theme` | `b0733091fb4a460d7ce8e0750276bdb7dc71409fbf0fa7e6103dbc8eb3a8a928` |
| `Notes.theme` | `72a47b913dc98d16c2f73edb8a61ed0d693a8015704528cffa430c3a69e06041` |
| `Olive Dunk.theme` | `d0c50311035a85c4bb49ac351f62c35e984e39f7c901db001f501738ce540538` |
| `Panic Mode.theme` | `50ed25235b3465424e239b95be5817069d0eb5f9f0c7dcfb3cc6279757ff7373` |
| `Print.theme` | `9e83d8376f37f23c3dd615cd3e5b4511cb37c51a37d14fa5548e542a6df21318` |
| `Red Graphite.theme` | `86b6e9ace9f83ae0a3ac5e1e4993808fed7afacb464af1bab3c4946d51cac50d` |
| `Rosé Pine Dawn.theme` | `cdc73c9fc660ea4a8b0611b8001a33e480f1fdafac9026e13953000920fc971a` |
| `Rosé Pine.theme` | `2c53a1e7e7cb13cbb5dd325c5f0f5f224ed0f294a7e068b99ecb6dbf7a2b1c54` |
| `Shibuya Jazz.theme` | `693919d60c3cd178ae08658dc424e2f7caf1e9939999624dfe3bc9bcfedaaae7` |
| `Shibuya Lo-fi.theme` | `7cc7a666bec612243eb38fda569ee22cc85f298cafefead7816e38c063b6a11e` |
| `Solarized Dark.theme` | `a05c2181ea380281c77ce1a5c5ea70a4c0e28d231e51be50cc9fd6897824423b` |
| `Solarized Light.theme` | `70a0c984cd9bd95e953704b23396740ab353cf959696d13e8d426728de08cec1` |
| `Tokyo Night Light.theme` | `40cbd15cb51e93fe607c76cdbcf9ecd42c38b365c6aefda3eded8ce0d4e19b14` |
| `Tokyo Night.theme` | `61c8ee99c7a1366c2bdca70330369513272e9dec582fe9def5e17aa385a66527` |
| `Toothpaste.theme` | `2998e7047a5e2ba9e3a84387cd6804d6f2439c2300dd0ab387974bdd3712ae4d` |

## Bjorn's themes

Themes written for Bjorn go here, below this line and outside the list above.
None yet.
