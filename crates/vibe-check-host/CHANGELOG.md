# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/getkono/vibe-check/releases/tag/binaries-v0.1.0) - 2026-09-02

### Features

- *(host)* make a leaf batch a type with distinct ids
- *(model)* make the decision clock a type, and ban the jiff wall clock
- *(model)* a checked LeafId, so the four hops cannot mangle it
- *(host)* a workflow run the adoption predicate can filter on
- *(host)* add the side-effect ports

### Fixes

- *(host)* mint the scheduler's fixture requirements through from_wire
- *(lint)* close the two routes around the wall-clock ban
- *(host)* give a scheduler leaf a typed lane, drop the unused serde

### Other

- merge origin/master into feat/27-canonical-digest
- *(host)* scope the scheduler test comments to what they prove
- Merge pull request #81 from getkono/feat/32-decision-time
- *(host)* forks are adoptable; the base repo is the authority
