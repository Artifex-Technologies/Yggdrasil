# yggdrasil-tzpicker: the time zone picker

Settings > Date & time and the installer's time zone page both show this picker. It has two parts:

- **A world map.** The time zone bands are drawn as thin lines, and the chosen zone is a filled band in the highlight colour with
  a soft glow. A ring marks the chosen city, and the UTC offsets run along the bottom of the map with the chosen one picked out.
  Clicking or tapping the map chooses the nearest city in the band you clicked: in western Kansas that is Chicago, though Denver
  is closer. Hovering over the map shows the zone and its local time. To get in close: pinch, Ctrl+wheel, double-click or the
  +/- buttons. Drag to move around. The plain mouse wheel is left to the page around the map, so it still scrolls.
- **A short list** beside the map, or below it when there is not much room. The tz database has about 400 zones people live in;
  the list puts them in about 80 rows, Windows-style. Zones in one part of the world that keep the same clock all through the
  coming year share a row. Denver, Boise and Ciudad Juarez, for example, are "Mountain Time (US & Canada)". Rows are
  grouped and sorted by their standard offset from UTC. Each row names its main cities, or the countries for a row named after
  its cities. You can search by city, country, zone name or offset ("UTC-07"), and accents do not matter.

What gets saved is always one tz database name:

- the row's main city;
- the city a search found;
- or the city nearest the click on the map.

The colour of the chosen zone is the palette's `highlight` (packages/yggdrasil-ui, orange by default). An operating system can
change it in its branding integration with `highlight = #rrggbb` (docs/INTEGRATIONS.md).

## Light on purpose

The map is drawn on the processor (tiny-skia) into one texture. It is drawn again only when what it shows changes: the view,
its size, the colours or the chosen band. Hovering and the labels are drawn by egui on top of it. Nothing animates while the map
sits still. The zones are read once from /usr/share/zoneinfo: zone.tab and iso3166.tab for the cities, countries and places, and
the tz database itself for each zone's offsets now and every 15 days over the coming year.

## The map data: Natural Earth

`data/world.bin` (110 KB) comes from [Natural Earth](https://www.naturalearthdata.com/), version 5.0.0, which is in the public
domain:

- `ne_50m_admin_0_countries`: the land;
- `ne_10m_time_zones`: the bands, each with its standard offset.

`examples/bake.rs` makes it as follows:

1. It simplifies the outlines (Douglas-Peucker, 0.04 degrees).
2. It rounds them to hundredths of a degree.
3. It leaves out Antarctica and islands too small to see.
4. It stores each point as the difference to the one before.

To make it again:

    cargo run --release --example bake -- ne_50m_admin_0_countries.shp ne_10m_time_zones.shp data/world.bin

I did not use timezone-boundary-builder's more detailed boundaries: they are under the ODbL, which would need share-alike
handling. Natural Earth's bands are a few years old, so the band drawn for a place whose offset changed since then can be off. The
time that gets set is unaffected: it always comes from the system's tz database.

## Tests

`cargo test`:

- the grouping, the order, the search and what each row says;
- the nearest-zone hit test (the band first, then the nearest city, across the date line);
- reading the map file back, including a damaged one;
- which band a point is in (Denver, Chicago, Berlin, Kolkata, the open Pacific);
- the view's arithmetic;
- that only the chosen band is drawn in the highlight colour.
