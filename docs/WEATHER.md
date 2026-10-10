# Weather

**Optional component** (`labyrinth-apps install weather`; part of the `everything` preset, not of the base
system). `packages/labyrinth-weather` (Rust). It does not claim to out-forecast the weather
services. What it does is combine several independent ones, show how much they
disagree, and learn from measurements which to trust at your location.

## Which services

| Service | Used? | Why / why not |
| --- | --- | --- |
| [Open-Meteo](https://open-meteo.com/en/docs) | **Yes** | Free for non-commercial use, no key. Serves NOAA GFS, ECMWF IFS and DWD ICON, plus a 30-member ensemble and place search. Their terms are theirs; commercial use needs a paid key |
| [aviationweather.gov](https://aviationweather.gov) METAR | **Yes**, for measurements | Free, no key, worldwide airport observations; the "truth" forecasts are scored against |
| [api.weather.gov](https://www.weather.gov/documentation/services-web-api) (NWS) | Not yet | Free, no key, needs a User-Agent; `/points` -> grid forecast and alerts. Verified reachable. US coverage is the likely limit (the page does not state it). The natural next addition: official alerts |
| AccuWeather, The Weather Company / Weather Channel, OpenWeather, Tomorrow.io | No | Need an account or paid key, and the free tiers are rate-limited and restrict redistribution. A `Provider` trait is where they would go if you bring a key. Not looked into in detail |

Everyone gets different numbers because the services run different models. Averaging
independent models usually beats any single one, and the *spread* between them is real
information about how uncertain the forecast is.

## What it does

- **Consensus:** the weighted mean of the three models for each hour, with their range.
- **Agreement** (high / medium / low): from how far apart the models are and how wide the
  ensemble is. The thresholds (2 and 4 degrees C) are heuristic labels, **not calibrated
  probabilities**.
- **Ensemble:** the 30-member GFS ensemble contributes the uncertainty width. It is a
  different model run from the three, so its absolute range can sit off to one side of
  their average; only its width is shown (as "+/-").
- **Learning:** `labyrinth-weather record` (hourly, via a systemd user timer) stores each
  model's predictions for the next 24 hours and the nearest station's measured
  temperature. `labyrinth-weather score` compares them: average error and bias per model.
  A model needs at least ~24 scored predictions before its weight changes anything, and
  weights only apply when at least two models qualify. Weight = 1 / (error + 0.5),
  scaled to average 1.
- **Never guesses your location.** Set it once (`set-location "City"` uses Open-Meteo's
  place search; `set-location --coords LAT LON`). Only those coordinates are sent out.

## Limits, stated plainly

- The accuracy learning needs **days of recorded data** before it does anything, and
  until then models are weighted equally (the tooltip says which).
- The measured temperature comes from the **nearest airport** (within ~60 km), not your
  yard; coasts, hills and cities differ from airports.
- It scores **temperature only**. Rain chance is shown but not verified.
- The icon is a **guess from rain chance and time of day**; there is no cloud-cover data.
- The 1-hour forecast steps and "now" are matched by local time using the forecast's own
  UTC offset.
- The weighting is a simple, sensible scheme, not a proven improvement; whether it helps
  at your location is exactly what `score` will show once there is data.

## Commands

    labyrinth-weather set-location "Your city"      # or: --coords LAT LON [NAME]
    labyrinth-weather units metric|us
    labyrinth-weather forecast [HOURS]               # table, every 3 hours
    labyrinth-weather bar                            # one line of Waybar JSON
    labyrinth-weather record                         # what the hourly timer runs
    labyrinth-weather score                          # accuracy per model

## Status

34 unit tests, including parsing of **real** API responses (saved in `tests/fixtures/`;
the ensemble and multi-model fixtures were trimmed to a few hours). Run live on Windows
against the real services at a neutral test point: forecast, bar output, recording and
the no-location path all worked. Not built on Linux; no PKGBUILD run; the recording
timer and the bar have never run together; the learning has never seen real history.

Not built: a graphical weather window (the bar tooltip and `forecast` table are the
display for now), NWS alerts, a radar view, and any provider beyond the two above.
