-- keep-clear.lua: where a floating window goes so that it keeps clear of the bar and the dock, the room they reserve on
-- their screen. Pure functions with no Hyprland calls: yggdrasil.lua runs them on window events, and
-- packages/session/tests/keep-clear.lua checks them with plain Lua.
local M = {}

-- One axis: a window from p, len long, kept inside lo..hi (shortened when it does not fit).
local function fit_axis(p, len, lo, hi)
  if len > hi - lo then len = hi - lo end
  if p + len > hi then p = hi - len end
  if p < lo then p = lo end
  return p, len
end

local function round(v) return math.floor(v + 0.5) end

-- The window at x, y, w x h on screen m (Hyprland's monitor: x, y, width and height in pixels, scale, transform, reserved =
-- { top, right, bottom, left } in layout pixels). Returns the new x, y, w, h, or nil when it may stay where it is.
-- Only edges with something reserved on them count (a window may still hang off a bare screen edge), and a window that
-- covers its whole screen exactly (a borderless game or a video) is left alone.
function M.fit(x, y, w, h, m)
  local s = (m.scale and m.scale > 0) and m.scale or 1
  local r = m.reserved or {}
  local top, right, bottom, left = r.top or 0, r.right or 0, r.bottom or 0, r.left or 0
  local sw, sh = m.width / s, m.height / s
  if (m.transform or 0) % 2 == 1 then sw, sh = sh, sw end
  local mx, my = m.x or 0, m.y or 0
  if math.abs(x - mx) <= 1 and math.abs(y - my) <= 1 and math.abs(w - sw) <= 1 and math.abs(h - sh) <= 1 then return nil end
  local x0, x1, y0, y1 = mx + left, mx + sw - right, my + top, my + sh - bottom
  if x1 - x0 < 100 or y1 - y0 < 100 then return nil end
  local nx, ny, nw, nh = x, y, w, h
  if (top > 0 and y < y0) or (bottom > 0 and y + h > y1) then ny, nh = fit_axis(y, h, y0, y1) end
  if (left > 0 and x < x0) or (right > 0 and x + w > x1) then nx, nw = fit_axis(x, w, x0, x1) end
  nx, ny, nw, nh = round(nx), round(ny), round(nw), round(nh)
  if nx == x and ny == y and nw == w and nh == h then return nil end
  return nx, ny, nw, nh
end

return M
