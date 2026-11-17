# 3: the core's home folder is ~/.config/contr-core

- The neutral default folder of the core's root is `~/.config/contr-core` (owner, 2026-11-17: "Core
  everywhere: folder ~/.config/contr-core, Settings says Core"). It replaces the folder name of decision 266
  (`~/.config/contr-backend`). The rest of 266 stays: HOME, else USERPROFILE; `--home`, `CONTR_HOME`,
  `HERDR_APP_HOME` and the installed place come before the default; the owner runs the move.
- The default root is `~/.config/contr-core` when it exists, else `~/.config/contr-backend` when it exists,
  else `~/.config/herdr-app/herdr-contr` when it exists, else `~/.config/contr-core`. No machine is known
  to have `~/.config/contr-backend`; this one fallback keeps such a machine working.
- `herdr-contr-feed home move [--dry-run]` moves `~/.config/contr-backend` when it is there, else
  `~/.config/herdr-app/herdr-contr`, to `~/.config/contr-core`, and leaves a link at the old path.
- Help texts and docs name the product "the core". Code identifiers, the flag `--backend`, the programs,
  the plugin, the environment variables, the zip and the tags keep their names.
