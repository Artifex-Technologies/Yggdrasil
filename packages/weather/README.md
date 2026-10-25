# yggdrasil-weather

Weather for the Yggdrasil top bar: the temperature from several independent models with an honest
uncertainty range, and rain for the next two hours in 15-minute steps.

## Your place

No terminal needed: click the weather in the top bar, then Settings on its card. The card (drawn by the panel, like every
top bar add-on; no window) offers three ways to know where you are:

| Choice | How | What leaves this computer |
|---|---|---|
| From time zone (the default) | the main city of the system's time zone (zone1970.tab), worked out each time | nothing (rough: a time zone can span hundreds of kilometres) |
| Automatic | the Wi-Fi networks NetworkManager last saw (no new scan), looked up at [beaconDB](https://beacondb.net/), an open, non-profit location database; checked again every few hours by `watch`, so a laptop follows you | the networks' IDs (BSSIDs, not names), their signal strength and your IP address, to beaconDB, only when it checks; the result is rounded to about 1 km |
| Chosen place | type a city in the card's search box and press Enter (Open-Meteo's place search), then pick it; the last pick is remembered, so you can switch back to it | the name you type, when you press Enter (nothing while typing) |

Units (Celsius or Fahrenheit) are on the same card. What the card is showing (settings, the search) lives in the runtime
folder and is forgotten after ten minutes. From a terminal, the same choices are
`yggdrasil-weather locate` (automatic, now), `set-location "Your city"` or `set-location --coords LAT LON [NAME]`,
and `units metric|us`.

## Commands

| Command | What it does |
|---|---|
| `panel` | One JSON object for the Yggdrasil panel's bar add-on slot (`weather.conf`, kind `bar-item`): the temperature and, when rain is coming, "rain in 25 min"; while it rains, the rate in mm/h. The card lists the models' range, the nearest measured temperature, and each 15-minute step when any rain is expected. |
| `press ID [TEXT]` | A press on the card, run by the panel (`act` in `weather.conf`): `view:settings`, `view:normal`, `mode:auto` (asks beaconDB, only then), `mode:timezone`, `mode:manual`, `search TEXT` (only stores it), `search-go`, `pick:N`, `units:metric`, `units:us`. The panel then runs `panel` again, which reads the forecast from a 10-minute cache (`~/.cache/yggdrasil/weather/`), so the card answers at once. |
| `watch` | Run every 10 minutes by `yggdrasil-weather-nowcast.timer`: one desktop notification when rain is expected to start within 30 minutes ("Rain in about 20 min", with its intensity and when it should be dry again). A shower is announced once, even when its expected start moves by a step or two. |
| `locate` | Switch to the automatic place and find it now (beaconDB). |
| `bar` | One line of Waybar-style JSON (text and tooltip). |
| `forecast [HOURS]` | The next hours in a terminal table. |
| `record`, `score` | Store each model's predictions and the nearest airport's measurements (hourly, `yggdrasil-weather-record.timer`), and show how accurate each model has been. Once enough history exists, the models are weighted by that accuracy. |

## Sources

- Temperature: NOAA GFS, ECMWF IFS and DWD ICON through [Open-Meteo](https://open-meteo.com/) (free for
  non-commercial use, no key), plus a 30-member GFS ensemble for the spread.
- Rain nowcast: Open-Meteo's `minutely_15` precipitation. In North America that is NOAA's HRRR, in much of
  Europe the regional high-resolution models; elsewhere it is filled from the hourly global models, so it is
  coarser there. Radar extrapolation (KNMI for the Netherlands, DWD for Germany, as in
  [omarchy-weather-nowcast](https://github.com/Hizoul/omarchy-weather-nowcast), whose rain view this follows: the idea
  only, written anew here, no code taken from it, as it is GPL-3.0)
  is not used: it covers only those two countries.
- Measured temperatures: airport reports (METAR) from aviationweather.gov.

These services receive the coordinates of the place in use (rounded, for the automatic one), and nothing else.

## Files

- `~/.config/yggdrasil/weather.json`: the place, how it was found (`mode`: manual, timezone or auto) and units. Without
  it, the time zone's city is used and nothing is written.
- `~/.local/share/yggdrasil/weather/`: recorded predictions and measurements, and `nowcast-state.json` (the
  last rain start that was announced).
