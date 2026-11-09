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

if failed > 0 then
  io.stderr:write(failed .. " failed\n")
  os.exit(1)
end
print("keep-clear: all checks pass")
