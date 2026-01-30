---
date: 2025-12-19
categories:
  - Announcements
---

# Introducing Bundlebase

Databases are always the big, scary apps that do everything. And as they morphed into data warehouses and data lakes and data whatevers, they have just gotten to be more.

But for so much, we just want to easily work with some data we have lying around. Some CSV data, parquet files, or data behind an API.

We want to get a bit of data into the shape we want and share it with others, without all the complexity that databases bring.

The goal of Bundlebase is to be "docker, but for data". 

You package up your data into a container that you can work with in a constant way--regardless of the underlying storage formats.

You can share that data with others who can use it as is or build new containers on top of it.

What I actually want is something closer to `docker pull` than `first, you fire up postresql...`. You grab a bundle, and it just works -- you can query it with
SQL, pull it into pandas, filter and transform it. The data comes with its structure and history already attached. No
setup, no guessing.

That's the idea behind Bundlebase.

## What it does today

Bundlebase is a data processing library with a Rust core and Python bindings. Here's what it looks like in practice:

```python
import bundlebase

c = await bundlebase.create()
await c.attach("sales_data.parquet")
await c.filter("region = 'US'")
await c.remove_column("internal_notes")
await c.commit("US sales, cleaned")

df = await c.to_pandas()
```

You attach data from files (Parquet, CSV, JSON), apply transformations, and commit snapshots. The commit history tracks
how the data evolved -- like git for your data pipeline.

The part I'm most interested in long-term is the sharing side. I want someone to be able to publish a bundle and have
someone else pull it down and immediately start working with it. No "download this file, then run this script, then
install these dependencies" -- just point at a bundle and go. We're not there yet, but that's the direction.

## Following along

This blog is where I'll post updates -- release notes, technical decisions, things I'm figuring out as I go. The code is
at [github.com/nvoxland/bundlebase](https://github.com/nvoxland/bundlebase) if you want to look around.

If you have questions or ideas, let me know. I'm figuring a lot of this out as I build it.
