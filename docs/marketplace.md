# The marketplace

The marketplace shares ready-made assistants, workflows, prompts and the
rest as **listings**: [bundles](bundles.md) with a title, tags, versions
and ratings. Every gai server has one, and can install from other gai
servers' marketplaces, called **registries**.

## Installing

**Marketplace** in the dashboard shows this server's listings and, in a
tab each, those of the registries you added. Open one to see what it
holds (assistants, workflows, prompts, personas, skills, connected apps,
knowledge), the models it uses, its versions and reviews.

**Check what it needs** lists the keys and models this server is missing,
without installing anything. **Install** saves its items here; items that
already exist get a new version (their history stays) unless you turn on
**Keep what's already here**. Bundles never carry keys or tokens: set the
ones it names on this server.

## Publishing

**Publish** picks assistants, workflows and prompts from this server and
bundles them with everything they use, then lists the bundle with a title,
description and tags. Publishing under the same name again makes a new
version, with a note on what's new. **Unpublish** removes a listing and
its bundles; what people already installed stays with them.

## Rating

Anyone with the run scope rates a listing 1 to 5 stars with an optional
comment; rating again replaces their rating. Ratings of another
registry's listing are sent there, made with the key this server holds
for it.

## Registries

Add another gai server under **Registries** with its address and a key
made on that server (`gai keys create --scope read` to browse and
install, `--scope run` to also rate). Keys are kept on this server and
never shown again. Listings, details and bundles are fetched from the
registry when you look at them; nothing is copied until you install.

A team can run one gai server as its registry: publish there, and add it
as a registry everywhere else.

## API

| | |
|---|---|
| `GET /v1/marketplace/listings` | This server's listings (what other servers browse) |
| `POST /v1/marketplace/listings` | Publish: `{name, title, description, tags, note, items: {agents, workflows, prompts, …}}` (`items` as for `/v1/bundles/export`) |
| `GET /v1/marketplace/listings/{name}` | A listing with its versions and reviews |
| `GET /v1/marketplace/listings/{name}/bundle` | Its bundle (`?version=`) |
| `DELETE /v1/marketplace/listings/{name}` | Unpublish |
| `POST /v1/marketplace/listings/{name}/ratings` | `{stars, comment}` |
| `GET\|PUT /v1/marketplace/registries` | Other registries: `[{name, url, token}]`; one sent without a token keeps its token |
| `GET /v1/marketplace/browse?registry=&q=` | Listings here (no registry) or in a registry, with search |
| `GET /v1/marketplace/browse/{registry}/{name}` | A listing's details, from wherever it is (`local` for here) |
| `POST /v1/marketplace/browse/{registry}/{name}/ratings` | Rate it there |
| `POST /v1/marketplace/install` | `{registry, name, version, skip_existing, dry_run}`: install, or see what it needs |

Publishing, installing and changing registries need an admin key, like
bundles; browsing needs read, and rating needs run.
