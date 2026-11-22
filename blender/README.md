# RedForge for Blender

`redforge_blender.zip` is the Blender add-on RedForge's installers ship: install it in Blender 5.3 or newer with
Preferences > Get Extensions > Install from Disk. It replaces the RedForge Bridge add-on and Sollumz RDR.

It's a fork of [Sollumz RDR](https://github.com/Sollumz/Sollumz-RDR) by Skylumz, Colton and alexguirre, and so under
the **GPL-3.0-or-later**, unlike the rest of this repository (MIT or Apache-2.0): a separate work that RedForge only
talks to, over a local socket (`crates/rf-bridge`). The .zip is its complete source, with its `LICENSE`.

The add-on is developed in its own repository; its `build.sh -o <this folder>/redforge_blender.zip` (`build.ps1 -Out` on Windows) makes the .zip
here again.
