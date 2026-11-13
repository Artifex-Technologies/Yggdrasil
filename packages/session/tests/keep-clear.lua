-- lua packages/session/tests/keep-clear.lua: checks where keep-clear.lua puts windows (plain Lua, no Hyprland).
local here = (arg and arg[0] or ""):match("^(.*)/[^/]*$") or "."
local kc = dofile(here .. "/../hyprland/keep-clear.lua")

local failed = 0
local function check(name, want, ...)
  local got = { ... }
  local ok = (want == nil and got[1] == nil)
  if want ~= nil then
    ok = #got == 4
    for i = 1, 4 do ok = ok and got[i] == want[i] end
  end
  if not ok then
    failed = failed + 1
    io.stderr:write(string.format("FAIL %s: got %s\n", name, table.concat((function()
      local t = {} for i = 1, #got do t[i] = tostring(got[i]) end return t end)(), " ")))
  end
end

-- the iPad screen: 2420x1668 at scale 2 (1210x834 to lay out), the bar 28 high, the dock 70
local ipad = { x = 0, y = 0, width = 2420, height = 1668, scale = 2.0, transform = 0,
  reserved = { top = 28.0, right = 0.0, bottom = 70.0, left = 0.0 } }
-- the big screen to its right, bar only
local wide = { x = 1210, y = 0, width = 3440, height = 1440, scale = 1.0, transform = 0,
  reserved = { top = 28.0, right = 0.0, bottom = 0.0, left = 0.0 } }

-- 2026-11-08: VS Code came back from maximize 1210x806 under the dock; it ends at the dock's edge
check("VS Code back from maximize", { 0, 28, 1210, 736 }, kc.fit(0, 28, 1210, 806, ipad))
-- a window that fits is moved up above the dock, keeping its size
check("small window over the dock", { 300, 464, 600, 300 }, kc.fit(300, 500, 600, 300, ipad))
-- under the bar: moved down
check("under the bar", { 100, 28, 500, 400 }, kc.fit(100, 10, 500, 400, ipad))
-- clear already: left alone
check("clear", nil, kc.fit(100, 100, 500, 400, ipad))
check("touching the dock's edge", nil, kc.fit(0, 28, 1210, 736, ipad))
-- the whole screen exactly (a borderless game): left alone
check("borderless full screen", nil, kc.fit(0, 0, 1210, 834, ipad))
-- hanging off a bare edge (no bar or dock there): left alone
check("off the left edge", nil, kc.fit(-200, 100, 500, 400, ipad))
-- the big screen has no dock: only the bar counts
check("tall window, no dock", nil, kc.fit(1300, 28, 2000, 1412, wide))
check("under the bar of the big screen", { 1300, 28, 2000, 1412 }, kc.fit(1300, 0, 2000, 1412, wide))
-- taller than the free room under the bar: shortened
check("taller than the room", { 1300, 28, 2000, 1412 }, kc.fit(1300, 0, 2000, 1440, wide))
-- the dock on the left side
local side = { x = 0, y = 0, width = 1920, height = 1080, scale = 1.0, transform = 0,
  reserved = { top = 28.0, right = 0.0, bottom = 0.0, left = 70.0 } }
check("left dock", { 70, 100, 800, 600 }, kc.fit(10, 100, 800, 600, side))
check("wider than the room beside a left dock", { 70, 100, 1850, 600 }, kc.fit(0, 100, 1920, 600, side))
-- a turned screen swaps width and height
local turned = { x = 0, y = 0, width = 1920, height = 1080, scale = 1.0, transform = 1,
  reserved = { top = 28.0, right = 0.0, bottom = 70.0, left = 0.0 } }
check("turned screen", { 0, 28, 1080, 1822 }, kc.fit(0, 28, 1080, 1900, turned))

-- back from maximize: a window as big as the free room gets a normal size (three quarters, centred)
check("VS Code's old size under the dock", { 151, 120, 908, 552 }, kc.normal(0, 28, 1210, 806, ipad))
check("Vivaldi at the maximized size", { 151, 120, 908, 552 }, kc.normal(0, 29, 1208, 734, ipad))
check("a normal window keeps its size", nil, kc.normal(200, 100, 700, 500, ipad))
check("wide but short keeps its size", nil, kc.normal(0, 28, 1210, 400, ipad))
check("the whole screen (a game) is left alone", nil, kc.normal(0, 0, 1210, 834, ipad))
check("big screen", { 1640, 205, 2580, 1059 }, kc.normal(1210, 28, 3440, 1412, wide))

-- as a window opens: one bigger than the free room is made to fit it, less the gap of 8, and centred; one that fits keeps its
-- size. The laptop's 1920x1200 panel at scale 1.5 (1280x800 to lay out), the bar 28 high, the dock 70.
local laptop = { x = 0, y = 0, width = 1920, height = 1200, scale = 1.5, transform = 0,
  reserved = { top = 28.0, right = 0.0, bottom = 70.0, left = 0.0 } }
-- 2026-11-13: VS Code's size kept from scale 1 (1208x806) is taller than the 702 the laptop leaves free
check("kept size from before the scale changed", { 36, 36, 1208, 686 }, kc.open(100, 50, 1208, 806, laptop, 8))
-- a pop-up wider than the screen (Hyprland centred it, hanging off both bare edges): as wide as the room, centred
check("wider than the screen", { 8, 179, 1264, 400 }, kc.open(-110, 200, 1500, 400, laptop, 8))
check("bigger both ways", { 8, 36, 1264, 686 }, kc.open(-200, -100, 1680, 1000, laptop, 8))
-- a normal dialog keeps its size and place
check("a normal dialog is left alone", nil, kc.open(374, 311, 532, 177, laptop, 8))
check("as big as the free room exactly is left alone", nil, kc.open(0, 28, 1280, 702, laptop, 8))
-- one that fits but reaches under the dock is only moved above it, as before
check("fits, but under the dock", { 100, 430, 600, 300 }, kc.open(100, 500, 600, 300, laptop, 8))
-- the whole screen exactly (a borderless game or a video) is left alone
check("borderless full screen as it opens", nil, kc.open(0, 0, 1280, 800, laptop, 8))
-- scale 2 (960x600 to lay out): a trust dialog 560 high in 502 free
local laptop2 = { x = 0, y = 0, width = 1920, height = 1200, scale = 2.0, transform = 0,
  reserved = { top = 28.0, right = 0.0, bottom = 70.0, left = 0.0 } }
check("tall dialog at scale 2", { 180, 36, 600, 486 }, kc.open(180, 20, 600, 560, laptop2, 8))
-- a screen with nothing reserved: the gap all round
local bare = { x = 1280, y = 0, width = 1280, height = 800, scale = 1.0, transform = 0, reserved = {} }
check("no bar or dock", { 1288, 8, 1264, 784 }, kc.open(1220, -50, 1400, 900, bare, 8))
-- turned into a tablet (portrait, 800x1280 to lay out): too wide
local portrait = { x = 0, y = 0, width = 1920, height = 1200, scale = 1.5, transform = 1,
  reserved = { top = 28.0, right = 0.0, bottom = 70.0, left = 0.0 } }
check("portrait screen", { 8, 216, 784, 806 }, kc.open(-204, 216, 1208, 806, portrait, 8))
-- no gap given: the free room itself
check("no gap", { 0, 28, 1280, 702 }, kc.open(-110, 0, 1500, 900, laptop))

if failed > 0 then
  io.stderr:write(failed .. " failed\n")
  os.exit(1)
end
print("keep-clear: all checks pass")
