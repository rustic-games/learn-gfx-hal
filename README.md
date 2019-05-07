[![License:Apache2](https://img.shields.io/badge/License-Apache2-green.svg)](https://www.apache.org/licenses/LICENSE-2.0)
[![AppVeyor](https://ci.appveyor.com/api/projects/status/39wvbxxstqjd2vi8?svg=true)](https://ci.appveyor.com/project/Lokathor/learn-gfx-hal)
[![travis.ci](https://travis-ci.org/Lokathor/learn-gfx-hal.svg?branch=master)](https://travis-ci.org/Lokathor/learn-gfx-hal)

[![gfx-hal:0.1](https://img.shields.io/badge/gfx--hal-0.1-blue.svg)](https://docs.rs/gfx-hal)

# Project Status

We're in long term maintenance mode!

This teaches you how to use `gfx-hal-0.1.0`, but 99% of the world shouldn't be
using `gfx-hal` directly in the first place. It's absolutely important to know
what's going on, but then you should move to a higher level API. Which API?
Well, right now [rendy](https://github.com/omni-viral/rendy) is your best bet.
Is there a book you can read for that once you're done with this book? Thanks to
`termhn`, [yes there is](https://github.com/termhn/learn-rendy).

When `gfx-hal` puts out new releases the tutorials will be updated so that
they always work with the latest release of `gfx-hal`, and I'll accept
submissions if people want to add more examples and the lessons that go with
them, but I don't plan on adding new lessons myself.

# learn-gfx-hal

Step by step tutorials for using the [gfx-hal](https://github.com/gfx-rs/gfx)
crate.

The tutorials all target the current version _on crates.io_, not on the master
branch of git. At the time of me writing this that means `0.1.0`.

This is _not_ intended to be a library crate for you to just import into your
own projects. It is a series of examples and explanations for you read and learn
from.

* The lessons are in the `book/` directory in markdown form. They can be
  rendered to HTML with [mdbook](https://github.com/rust-lang-nursery/mdBook),
  and the [GitHub Pages site](https://lokathor.github.io/learn-gfx-hal/) for
  this repository hosts a rendered version of the master branch.
* The fully working examples are in the `examples/` directory. Each example
  attempts to be a single file that works on its own, so in some cases the code
  style _isn't_ quite what you'd want on a full project (eg: shader code
  contained in string literals instead of saved in separate files).

The code examples are **not** meant to be taken alone. There is effectively zero
explanation within the code files themselves. You are **absolutely** encouraged
to read the lesson text that goes with each example.

## Learning More

The lessons here mostly focus on the particulars of how to get the output you
want with `gfx-hal`. Some information on graphical techniques in general will
eventually be covered, but I can only cover so much material so fast.

If you want a whole lot more about general graphical stuff there's a large
number of free books on the
[RealTimeRendering.com](http://www.realtimerendering.com/#books-small-table)
website. It's mostly their older editions, but the math for ray tracing or
lighting or whatever else is all is still accurate enough. Once you know enough
about how `gfx-hal` does things you should be able to pick any book off of that
list and convert the techniques shown (usually C++ and OpenGL) over into your
own code (Rust and `gfx-hal`).

## Requirements

Uses [shaderc-rs](https://github.com/google/shaderc-rs), please follow [their
setup instructions](https://github.com/google/shaderc-rs#setup).

* Regarding `msys2` on Windows: Note that the _first_ time you run the `pacman`
  command they list _it doesn't install the packages_. Instead it actually just
  installs the latest pacman and msys files. You have to then run the `pacman`
  command again to make it actually download the stuff (and if you ran it from
  within the msys2 terminal you have to close that terminal and open a new one).
  * Yes, that is totally stupid, but it is also real advice that you must follow
    in this, the year of our lord two thousand and nineteen, if you want to
    program 3D graphics programs on windows.
* When they say "the msys2 mingw64 binary path" they mean `C:\msys64\usr\bin`
  and `C:\msys64\mingw64\bin` (assuming that you installed to `C:\msys64`).

## Contribution

This repo is Apache 2 licensed and all of your contributions must be made under
that license.

## Disclaimer

Hello. The primary ~~author~~ quixotic fool of this project is me, Lokathor. I
explain things in a way that beginners can hopefully understand by not actually
knowing anything about it myself. I get advice and guidance from the
[gfx-rs](https://github.com/gfx-rs) team when writing each lesson, but the final
editorial sign off is mine, and and crazy or stupid opinions that you find here
are _mine alone_, not any fault of theirs.

If there are mistakes that have crept in anywhere please file an issue. You can
also attempt to contact me interactively on the `#gamedev` channel of the
[community discord](https://bit.ly/rust-community).
