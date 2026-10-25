# Contributing to Moo

Moo is developed at [allisonhere/moo](https://github.com/allisonhere/moo). Use this repository for Moo bugs, documentation corrections, and proposed changes.

## Report a bug

Search existing issues first. Include the Moo version, operating system, terminal, shortest reproduction, expected behavior, and actual behavior. Include only the relevant log excerpt and remove secrets.

For features or larger changes, discuss the intended behavior with the project owner before implementing it. A report or proposal does not guarantee acceptance.

## Prepare a change

Read [AGENTS.md](AGENTS.md). Keep each change focused, explain its behavior, and preserve unrelated work. The installed CLI and source code are the authority for commands and configuration. Put current documentation in `docs/next`; the wiki is generated from those sources by `scripts/sync_wiki.py`.

Moo documentation should lead with Moo. Keep license and copyright notices, link to the [credits](https://github.com/allisonhere/moo/wiki/Docs-credits), and preserve real compatibility identifiers. Do not rename runtime keys, filenames, or protocol values as part of a prose edit.

## Verify your work

Use the repository recipes:

```sh
just test
just check
```

Run `just check` before committing unless the owner explicitly accepts narrower validation. Explain any failing or unavailable checks. Rust code must not use `unwrap()` in production; use `tracing` for logging and keep platform-specific behavior in `src/platform/`.

For documentation and wiki-generator changes, run:

```sh
python3 -m unittest scripts.test_sync_wiki scripts.test_docs_identity
just docs-contract-test
```

Regenerate the wiki in a separate checkout, then run the sync script with `--check`. Never overwrite a handwritten wiki page through the generator.

## Commit and review

Use lowercase conventional commit subjects, without emojis or AI co-author lines. Propose the commit message and get alignment before committing. Add `refs #<issue-number>` in the body when applicable. Do not merge pull requests on the owner's behalf.

## Upstream contributions

Moo builds on Herdr, but changes here are not submissions to that project. Any issue, pull request, or branch pushed to `herdrdev/herdr` must independently follow its current contribution and authorization rules. Moo ownership grants no authority in the upstream repository.
