# yggdrasil-weather

Weather for the Yggdrasil top bar: the temperature from several independent models with an honest
uncertainty range, and rain for the next two hours in 15-minute steps.

## Set up

```
yggdrasil-weather set-location "Your city"        # or: set-location --coords LAT LON [NAME]
yggdrasil-weather units us                         # or metric (the default)
```

## Commands

| Command | What it does |
|---|---|
| `panel` | One JSON object for the Yggdrasil panel's bar add-on slot (`weather.conf`, kind `bar-item`): the temperature and, when rain is coming, "rain in 25 min"; while it rains, the rate in mm/h. The card lists the models' range, the nearest measured temperature, and each 15-minute step when any rain is expected. |
| `watch` | Run every 10 minutes by `yggdrasil-weather-nowcast.timer`: one desktop notification when rain is expected to start within 30 minutes ("Rain in about 20 min", with its intensity and when it should be dry again). A shower is announced once, even when its expected start moves by a step or two. |
| `bar` | One line of Waybar-style JSON (text and tooltip). |
| `forecast [HOURS]` | The next hours in a terminal table. |
| `record`, `score` | Store each model's predictions and the nearest airport's measurements (hourly, `yggdrasil-weather-record.timer`), and show how accurate each model has been. Once enough history exists, the models are weighted by that accuracy. |

## Sources

- Temperature: NOAA GFS, ECMWF IFS and DWD ICON through [Open-Meteo](https://open-meteo.com/) (free for
  non-commercial use, no key), plus a 30-member GFS ensemble for the spread.
- Rain nowcast: Open-Meteo's `minutely_15` precipitation. In North America that is NOAA's HRRR, in much of
  Europe the regional high-resolution models; elsewhere it is filled from the hourly global models, so it is
  coarser there. Radar extrapolation (KNMI for the Netherlands, DWD for Germany, as in
  [omarchy-weather-nowcast](https://github.com/Hizoul/omarchy-weather-nowcast), which this rain view follows)
  is not used: it covers only those two countries.
- Measured temperatures: airport reports (METAR) from aviationweather.gov.

These services receive the coordinates of the place you set, and nothing else.

## Files

- `~/.config/yggdrasil/weather.json`: the place and units.
- `~/.local/share/yggdrasil/weather/`: recorded predictions and measurements, and `nowcast-state.json` (the
  last rain start that was announced).
