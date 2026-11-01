# osu-shelf 0.4.0 smoke test

1. **Version:** run `pacman -Q osu-shelf` (expect `0.4.0-1`) and `osu-shelf --version` (expect `osu-shelf 0.4.0`). Launch osu-shelf-gui.
2. **Minimum window:** shrink it as far as it goes. The sidebar becomes a compact top bar and the page starts at the top, with no big blank gap.
3. **Wide window:** widen or maximise. Find shows its search form beside a grid of cover cards; Settings splits into two columns. Resize back and forth; nothing should overlap or scroll sideways.
4. **Download ETA:** start a collection with enough maps to create a backlog (a few hundred; cancel after looking). While it downloads, the line shows counts, size and an ETA such as `under a minute left` or `about N min left`. Hover the ETA for mirror details.
5. **About:** open Settings → About. It should read `osu-shelf 0.4.0`. Click `Show third-party licences`, confirm the list appears, then Hide it.
