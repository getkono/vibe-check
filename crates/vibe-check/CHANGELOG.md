# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/getkono/vibe-check/releases/tag/binaries-v0.1.0) - 2026-09-02

### Features

- ship the action, the binary release chain, and the @vN policy
- *(host)* make a leaf batch a type with distinct ids
- *(model)* a checked LeafId, so the four hops cannot mangle it
- *(cli)* add the command surface, exit-code contract, and registration seam

### Fixes

- let release-plz consider the one package that ships
- *(cli)* make the crash report a write we are allowed to lose
- *(cli)* close the three holes a review found in the panic path
- *(cli)* keep the panic hatch out of release binaries
- *(cli)* keep the crash report when recording a panic
- *(scheduler)* name the leaf that panicked instead of dropping it
- *(cli)* make a panic exit 1 with a human verdict
- *(host)* give a scheduler leaf a typed lane, drop the unused serde

### Other

- merge origin/master into docs/14-readme
- Merge pull request #87 from getkono/feat/31-requirement-id-derive
- Merge pull request #72 from getkono/build/3-wire-vibe-check-diff
- keep implementation notes out of --help, and out of unverified claims
- restructure the package into a cargo workspace
- initialize project
