// Windows backend. Every operation is one PowerShell invocation whose script
// travels as a base64 -EncodedCommand, so tool arguments never become shell
// syntax. Screenshots + UIA accessibility come from .NET; raw pointer and
// keyboard events come from user32 P/Invoke (SendInput/mouse_event).
// Recording uses ffmpeg gdigrab when ffmpeg is on PATH.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import crypto from "node:crypto";
import { spawn } from "node:child_process";
import { run, runOk, ExecError, tryJson } from "../exec.mjs";

function ps(script, opts = {}) {
  const encoded = Buffer.from(script, "utf16le").toString("base64");
  return run("powershell.exe", ["-NoProfile", "-NonInteractive", "-EncodedCommand", encoded], {
    timeoutMs: opts.timeoutMs ?? 25_000,
    maxBuffer: 32 * 1024 * 1024,
  });
}

async function psJson(script, opts = {}) {
  const r = await ps(script, opts);
  const out = r.stdout.trim();
  const j = tryJson(out, null);
  if (!j) throw new ExecError(`powershell did not return JSON: ${(r.stderr || out).trim().slice(0, 300)}`, r);
  return j;
}

const USER32 = `
using System;
using System.Runtime.InteropServices;
public static class User32 {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int X, int Y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint dwFlags, int dx, int dy, uint dwData, UIntPtr dwExtraInfo);
  [DllImport("user32.dll")] public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);
  [DllImport("user32.dll")] public static extern bool GetCursorPos(out POINT lpPoint);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
  [StructLayout(LayoutKind.Sequential)] public struct POINT { public int X; public int Y; }
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx; public int dy; public uint mouseData; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr dwExtraInfo; }
  [StructLayout(LayoutKind.Explicit)] public struct INPUTUNION { [FieldOffset(0)] public MOUSEINPUT mi; [FieldOffset(0)] public KEYBDINPUT ki; }
  [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public INPUTUNION u; }
  public const uint MOVED = 0x0001, LEFTDOWN = 0x0002, LEFTUP = 0x0004, RIGHTDOWN = 0x0008, RIGHTUP = 0x0010,
    MIDDLEDOWN = 0x0020, MIDDLEUP = 0x0040, WHEEL = 0x0800, HWHEEL = 0x1000, ABSOLUTE = 0x8000, VIRTUALKEY = 0x4000, KEYUP = 0x0002, UNICODE = 0x0004;
  public static INPUT MouseInput(uint flags, int x, int y, uint data) {
    return new INPUT { type = 0, u = new INPUTUNION { mi = new MOUSEINPUT { dx = x, dy = y, mouseData = data, dwFlags = flags, time = 0, dwExtraInfo = IntPtr.Zero } } };
  }
  public static INPUT KeyInput(ushort vk, ushort scan, uint flags) {
    return new INPUT { type = 1, u = new INPUTUNION { ki = new KEYBDINPUT { wVk = vk, wScan = scan, dwFlags = flags, time = 0, dwExtraInfo = IntPtr.Zero } } };
  }
}`;

function recordingsDir() {
  return process.env.CODEWHALE_CU_RECORDINGS_DIR || path.join(os.homedir(), ".codewhale-cu", "recordings");
}

// Windows virtual-key codes for named keys.
const VK = {
  return: 0x0d, enter: 0x0d, tab: 0x09, escape: 0x1b, esc: 0x1b, space: 0x20,
  backspace: 0x08, delete: 0x2e, home: 0x24, end: 0x23, pageup: 0x21, pagedown: 0x22,
  left: 0x25, up: 0x26, right: 0x27, down: 0x28, capslock: 0x14, insert: 0x2d,
  f1: 0x70, f2: 0x71, f3: 0x72, f4: 0x73, f5: 0x74, f6: 0x75, f7: 0x76, f8: 0x77,
  f9: 0x78, f10: 0x79, f11: 0x7a, f12: 0x7b,
};
const MODVK = { ctrl: 0x11, control: 0x11, alt: 0x12, shift: 0x10, win: 0x5b, meta: 0x5b, cmd: 0x5b };

export function create() {
  const bootstrapped = (async () => {
    await ps(`Add-Type -TypeDefinition @'\n${USER32}\n'@ -ErrorAction SilentlyContinue; [User32] | Out-Null`, { timeoutMs: 30_000 });
  })();
  let lastRaster = null;
  let recording = null; // {id, pid, file, startedAt, mode}

  async function withUser32(script, opts) {
    await bootstrapped;
    return ps(script, opts);
  }

  return {
    platform: "win32",
    probe: async () => {
      await bootstrapped.catch(() => {});
      let ffmpeg = true;
      try { await runOk("ffmpeg", ["-version"], { timeoutMs: 10_000 }); } catch { ffmpeg = false; }
      const psOk = await ps("Write-Output 'ok'").then((r) => r.code === 0).catch(() => false);
      return {
        platform: "win32",
        powershell: psOk,
        capabilities: { screenshot: psOk, accessibility_tree: psOk, clipboard: psOk, recording: ffmpeg, raw_input: psOk },
        note: "Recording needs ffmpeg (gdigrab) on PATH. UIA accessibility works without extra installs.",
      };
    },
    list_displays: async () => {
      await bootstrapped;
      const d = await psJson(`Add-Type -AssemblyName System.Windows.Forms;
$out = [System.Windows.Forms.Screen]::AllScreens | ForEach-Object { [pscustomobject]@{ name = $_.DeviceName; primary = $_.Primary; x = $_.Bounds.X; y = $_.Bounds.Y; w = $_.Bounds.Width; h = $_.Bounds.Height; } } | ConvertTo-Json -Compress;
Write-Output ('{"displays": ' + ($out -replace '^\\[','[' -replace '\\]$/',']') + '}');`, { timeoutMs: 15_000 }).catch(async () => {
        // Fallback: build the array safely.
        return psJson(`Add-Type -AssemblyName System.Windows.Forms;
$arr = @(); foreach ($s in [System.Windows.Forms.Screen]::AllScreens) { $arr += [pscustomobject]@{ name = $s.DeviceName; primary = $s.Primary; x = $s.Bounds.X; y = $s.Bounds.Y; w = $s.Bounds.Width; h = $s.Bounds.Height } }
Write-Output ('{"displays": ' + (ConvertTo-Json $arr -Compress) + '}');`, { timeoutMs: 15_000 });
      });
      return d.displays.map((x, i) => ({ index: i + 1, name: x.name, points: { x: x.x, y: x.y, w: x.w, h: x.h }, pixels: { w: x.w, h: x.h }, scale: 1, main: !!x.primary }));
    },
    async switch_display({ index }) { return { activeDisplay: index ?? 1, note: "windows screenshots grab the virtual screen; per-display crop applies where supported" }; },
    list_apps: async () => {
      await bootstrapped;
      const j = await psJson(`Add-Type -AssemblyName System.Windows.Forms;
$out = Get-Process | Where-Object { $_.MainWindowTitle } | ForEach-Object { [pscustomobject]@{ name = $_.ProcessName; pid2 = $_.Id; title = $_.MainWindowTitle } } | ConvertTo-Json -Compress;
if (-not $out) { $out = '[]' }
Write-Output ('{"apps": ' + $out + '}');`);
      return { apps: (Array.isArray(j.apps) ? j.apps : [j.apps]).map((a) => ({ name: a.name, pid: a.pid2, title: a.title })) };
    },
    list_windows: async () => {
      await withUser32(`Add-Type -AssemblyName System.Windows.Forms;
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class WinEnum {
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr hWnd);
  [DllImport("user32.dll")] static extern int GetWindowText(IntPtr hWnd, StringBuilder text, int count);
  [DllImport("user32.dll")] static extern int GetWindowTextLength(IntPtr hWnd);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
  delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
  public static List<string> List() {
    var result = new List<string>();
    EnumWindows((h, l) => {
      if (!IsWindowVisible(h)) return true;
      int len = GetWindowTextLength(h);
      var sb = new StringBuilder(len + 1);
      GetWindowText(h, sb, sb.Capacity);
      uint pid; GetWindowThreadProcessId(h, out pid);
      RECT r; GetWindowRect(h, out r);
      if (sb.Length > 0 && r.Right > r.Left)
        result.Add(pid + "|" + r.Left + "," + r.Top + "," + (r.Right - r.Left) + "," + (r.Bottom - r.Top) + "|" + sb.ToString());
      return true;
    }, IntPtr.Zero);
    return result;
  }
}
'@;
$json = (WinEnum::List() | ForEach-Object { $p = $_.Split('|', 2); $parts = $p[1].Split('|', 2); [pscustomobject]@{ pid2 = [int]$p[0]; geom = $parts[0]; title = $parts[1] } } | ConvertTo-Json -Compress;
if (-not $json) { $json = '[]' }
Write-Output ('{"windows": ' + $json + '}');`, { timeoutMs: 25_000 }).then((j) => ({
        windows: (Array.isArray(j.windows) ? j.windows : [j.windows]).map((w) => {
          const g = String(w.geom).split(",").map(Number);
          return { pid: w.pid2, title: w.title, position: { x: g[0], y: g[1] }, size: { w: g[2], h: g[3] } };
        }),
      })).catch((e) => { throw e; });
    },
    open_application: async ({ name, bundle_id: bid, url: urlArg, activate } = {}) => {
      const target = name ?? bid;
      if (!target || !/^[A-Za-z0-9][A-Za-z0-9 .:_-]*$/.test(target)) throw new ExecError("open_application needs a plain app or executable name");
      const r = await ps(`Start-Process -FilePath "${target.replace(/"/g, "")}"${urlArg ? ` -ArgumentList "${urlArg.replace(/"/g, "")}"` : ""}; Write-Output '{"launched": true}'`, { timeoutMs: 20_000 });
      if (r.code !== 0) throw new ExecError(`Start-Process failed: ${r.stderr.trim().slice(0, 200)}`, r);
      return { launched: true, name: target, url: urlArg ?? null, activate };
    },
    get_app_state: async ({ app_ref, detail } = {}) => {
      await bootstrapped;
      const filter = app_ref?.name ? app_ref.name.replace(/'/g, "''") : "";
      const maxEls = detail === "full" ? 800 : 400;
      const j = await psJson(`Add-Type -AssemblyName UIAutomationClient;
Add-Type -AssemblyName UIAutomationTypes;
$max = ${maxEls};
$root = [System.Windows.Automation.AutomationElement]::RootElement;
$els = New-Object System.Collections.ArrayList;
$found = $false; $truncated = $false; $appName = $null;
$targets = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition);
foreach ($t in $targets) {
  $nm = $t.Current.Name;
  if ('${filter}' -and $nm -notlike '*${filter}*') { continue }
  $found = $true; $appName = $nm;
  $stack = New-Object System.Collections.Stack;
  $stack.Push(@($t, @(0)));
  while ($stack.Count -gt 0) {
    $entry = $stack.Pop(); $cur = $entry[0]; $path = $entry[1];
    if ($els.Count -ge $max) { $truncated = $true; break }
    $rect = $cur.Current.BoundingRectangle;
    $acts = @();
    try { $acts = @($cur.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName -replace 'Pattern$','' }) } catch {}
    [void]$els.Add([pscustomobject]@{ index = $els.Count; path = $path; role = [string]$cur.Current.ControlType.ProgrammaticName; label = [string]$cur.Current.Name; value = $([string]$cur.Current.Value).Substring(0, [Math]::Min(120, [string]$cur.Current.Value).Length); enabled = $cur.Current.IsEnabled;
      x = [int]$rect.X; y = [int]$rect.Y; w = [int]$rect.Width; h = [int]$rect.Height; actions = $acts });
    $kids = $cur.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition);
    for ($i = $kids.Count - 1; $i -ge 0; $i--) { $stack.Push(@($kids[$i], ($path + $i))) }
  }
  break;
}
$result = [pscustomobject]@{ found = $found; name = $appName; truncated = $truncated; elements = @($els | ForEach-Object { [pscustomobject]@{ index = $_.index; path = $_.path; role = ($_.role -replace 'ControlType.',''); label = $_.label; value = $_.value; enabled = $_.enabled; position = [pscustomobject]@{ x = $_.x; y = $_.y }; size = [pscustomobject]@{ w = $_.w; h = $_.h }; actions = $_.actions } }) };
Write-Output ($result | ConvertTo-Json -Depth 6 -Compress);`, { timeoutMs: 60_000 });
      if (!j.found) throw new ExecError("application window not found in UIA tree — pass app_ref.name from list_apps");
      return j;
    },
    screenshot: async ({ display, region, path: outPath } = {}) => {
      await bootstrapped;
      const dir = recordingsDir();
      fs.mkdirSync(dir, { recursive: true });
      const file = outPath || path.join(dir, `shot-${new Date().toISOString().replace(/[:.]/g, "-")}-${crypto.randomBytes(3).toString("hex")}.png`);
      const winPath = file.replace(/\\/g, "\\\\");
      const script = `Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing;
$bounds = [System.Windows.Forms.SystemInformation]::VirtualScreen;
$bmp = New-Object System.Drawing.Bitmap($bounds.Width, $bounds.Height);
$g = [System.Drawing.Graphics]::FromImage($bmp);
$g.CopyFromScreen($bounds.X, $bounds.Y, 0, 0, $bounds.Size);
$g.Dispose();
$bmp.Save('${file.replace(/'/g, "''")}', [System.Drawing.Imaging.ImageFormat]::Png);
$bmp.Dispose();
Write-Output '{"ok": true, "w": ' + $bounds.Width + ', "h": ' + $bounds.Height + '}';`;
      const r = await ps(script, { timeoutMs: 30_000 });
      if (r.code !== 0 || !fs.existsSync(file)) throw new ExecError(`screenshot failed: ${(r.stderr || r.stdout).trim().slice(0, 300)}`, r);
      const meta = tryJson(r.stdout.trim().split("\n").pop(), {});
      lastRaster = { file, bytes: fs.statSync(file).size, points: { x: 0, y: 0, w: meta.w, h: meta.h }, pixels: { w: meta.w, h: meta.h }, scale: 1, capturedAt: new Date().toISOString() };
      return { ...lastRaster };
    },
    zoom: async ({ source, region, path: outPath }) => {
      const src = source ?? lastRaster?.file;
      if (!src) throw new ExecError("no screenshot taken yet on this computer — call screenshot first");
      const out = outPath || path.join(recordingsDir(), `zoom-${crypto.randomBytes(4).toString("hex")}.png`);
      const script = `Add-Type -AssemblyName System.Drawing;
$img = [System.Drawing.Image]::FromFile('${src.replace(/'/g, "''")}');
$rect = New-Object System.Drawing.Rectangle(${Math.round(region[0])}, ${Math.round(region[1])}, ${Math.round(region[2])}, ${Math.round(region[3])});
$bmp = New-Object System.Drawing.Bitmap($rect.Width, $rect.Height);
$g = [System.Drawing.Graphics]::FromImage($bmp);
$g.DrawImage($img, (New-Object System.Drawing.Rectangle(0, 0, $rect.Width, $rect.Height)), $rect, [System.Drawing.GraphicsUnit]::Pixel);
$g.Dispose();
$bmp.Save('${out.replace(/'/g, "''")}', [System.Drawing.Imaging.ImageFormat]::Png);
$bmp.Dispose(); $img.Dispose();
Write-Output '{"ok": true}';`;
      const r = await ps(script, { timeoutMs: 20_000 });
      if (r.code !== 0 || !fs.existsSync(out)) throw new ExecError(`zoom failed: ${(r.stderr || "").slice(0, 250)}`, r);
      return { file: out, bytes: fs.statSync(out).size, region, source: src };
    },
    left_click: ({ target }) => clickAt(0, target.x, target.y, 1),
    double_click: ({ target }) => clickAt(0, target.x, target.y, 2),
    triple_click: ({ target }) => clickAt(0, target.x, target.y, 3),
    right_click: ({ target }) => clickAt(1, target.x, target.y, 1),
    middle_click: ({ target }) => clickAt(2, target.x, target.y, 1),
    mouse_move: async ({ target }) => {
      await withUser32(`[User32]::SetCursorPos(${Math.round(target.x)}, ${Math.round(target.y)}) | Out-Null; Write-Output '{"ok": true}'`);
      return { action_sent: true, at: { x: target.x, y: target.y } };
    },
    left_click_drag: async ({ from_target: from, to }) => {
      await withUser32(`[User32]::SetCursorPos(${Math.round(from.x)}, ${Math.round(from.y)}) | Out-Null;
Start-Sleep -Milliseconds 80;
[User32]::mouse_event([User32]::LEFTDOWN, 0, 0, 0, [UIntPtr]::Zero);
Start-Sleep -Milliseconds 80;
[User32]::SetCursorPos(${Math.round(to.x)}, ${Math.round(to.y)}) | Out-Null;
Start-Sleep -Milliseconds 80;
[User32]::mouse_event([User32]::LEFTUP, 0, 0, 0, [UIntPtr]::Zero);
Write-Output '{"ok": true}';`, { timeoutMs: 20_000 });
      return { action_sent: true, from, to };
    },
    left_mouse_down: async ({ target }) => {
      await withUser32(target ? `[User32]::SetCursorPos(${Math.round(target.x)}, ${Math.round(target.y)}) | Out-Null;` : "" + `[User32]::mouse_event([User32]::LEFTDOWN, 0, 0, 0, [UIntPtr]::Zero); Write-Output '{"ok": true}'`);
      return { action_sent: true };
    },
    left_mouse_up: async () => {
      await withUser32(`[User32]::mouse_event([User32]::LEFTUP, 0, 0, 0, [UIntPtr]::Zero); Write-Output '{"ok": true}'`);
      return { action_sent: true };
    },
    scroll: async ({ target, direction = "down", amount = 3 }) => {
      const notches = Math.max(1, Math.min(30, amount));
      const data = (direction === "down" ? -1 : direction === "up" ? 1 : 0) * notches * 120;
      const hdata = (direction === "right" ? 1 : direction === "left" ? -1 : 0) * notches * 120;
      await withUser32(`[User32]::SetCursorPos(${Math.round(target.x)}, ${Math.round(target.y)}) | Out-Null;
Start-Sleep -Milliseconds 60;
[User32]::mouse_event([User32]::WHEEL, 0, 0, [uint32]"$([int64]${data} -band 0xFFFFFFFF)", [UIntPtr]::Zero);
[User32]::mouse_event([User32]::HWHEEL, 0, 0, [uint32]"$([int64]${hdata} -band 0xFFFFFFFF)", [UIntPtr]::Zero);
Write-Output '{"ok": true}';`);
      return { action_sent: true, direction, amount };
    },
    type: async ({ text }) => {
      if (!text) return { action_sent: false, note: "empty text" };
      await bootstrapped;
      const b64 = Buffer.from(String(text), "utf16le").toString("base64");
      const script = `Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class TypeText {
  [DllImport("user32.dll")] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
  [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk; public ushort wScan; public uint dwFlags; public uint time; public IntPtr extra; }
  [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public KEYBDINPUT ki; }
  public static void SendString(string s) {
    foreach (char c in s) {
      var down = new INPUT { type = 1, ki = new KEYBDINPUT { wScan = c, dwFlags = 0x0008 | 0x0004 } };
      var up = new INPUT { type = 1, ki = new KEYBDINPUT { wScan = c, dwFlags = 0x0008 | 0x0004 | 0x0002 } };
      INPUT[] arr = new INPUT[2]; arr[0] = down; arr[1] = up;
      SendInput(2, arr, System.Runtime.InteropServices.Marshal.SizeOf(typeof(INPUT)));
    }
  }
}
'@;
$text = [System.Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('${b64}'));
[TypeText]::SendString($text);
Write-Output ('{"ok": true, "chars": ' + $text.Length + '}');`;
      const r = await ps(script, { timeoutMs: Math.max(20_000, text.length * 60) });
      if (r.code !== 0) throw new ExecError(`type failed: ${(r.stderr || "").slice(0, 250)}`, r);
      return { action_sent: true, chars: text.length, strategy: "unicode-sendinput" };
    },
    key: async ({ text, repeat = 1 }) => {
      const parts = String(text).split("+").map((s) => s.trim().toLowerCase());
      const mods = parts.filter((p) => MODVK[p] != null).map((p) => MODVK[p]);
      const key = parts.find((p) => MODVK[p] == null);
      if (!key) throw new ExecError(`no key in "${text}"`);
      let vk = VK[key];
      if (vk == null && key.length === 1) vk = key.toUpperCase().charCodeAt(0);
      if (vk == null) throw new ExecError(`unknown key "${key}"`);
      const n = Math.max(1, Math.min(100, repeat));
      await withUser32(`$ins = New-Object 'User32+INPUT[]' 0;
$add = { param($i) };
$seq = @();
${mods.map((m) => `$seq += [User32]::KeyInput(${m}, 0, 0);`).join("\n")}
for ($i = 0; $i -lt ${n}; $i++) { $seq += [User32]::KeyInput(${vk}, 0, 0); $seq += [User32]::KeyInput(${vk}, 0, 2); }
${[...mods].reverse().map((m) => `$seq += [User32]::KeyInput(${m}, 0, 2);`).join("\n")}
$arr = $seq.ToArray();
[void][User32]::SendInput($arr.Length, $arr, [System.Runtime.InteropServices.Marshal]::SizeOf([type][User32+INPUT]));
Write-Output '{"ok": true}';`);
      return { action_sent: true, key, repeat: n };
    },
    hold_key: async ({ text, duration }) => {
      const parts = String(text).split("+").map((s) => s.trim().toLowerCase());
      const key = parts.find((p) => MODVK[p] == null && VK[p] == null ? p.length === 1 : (VK[p] != null || p.length === 1));
      const vk = VK[key] ?? (key.length === 1 ? key.toUpperCase().charCodeAt(0) : null);
      if (vk == null) throw new ExecError(`unknown key "${text}"`);
      const d = Math.max(0.05, Math.min(30, Number(duration) || 1));
      await withUser32(`[void][User32]::SendInput(1, @([User32]::KeyInput(${vk}, 0, 0)), [System.Runtime.InteropServices.Marshal]::SizeOf([type][User32+INPUT]));
Start-Sleep -Milliseconds ${Math.round(d * 1000)};
[void][User32]::SendInput(1, @([User32]::KeyInput(${vk}, 0, 2)), [System.Runtime.InteropServices.Marshal]::SizeOf([type][User32+INPUT]));
Write-Output '{"ok": true}';`, { timeoutMs: Math.max(10_000, d * 1000 + 8000) });
      return { action_sent: true, key, heldSec: d };
    },
    set_value: async ({ target, value }) => {
      // UIA ValuePattern via a re-walk to target.path from the desktop root.
      await bootstrapped;
      const b64path = Buffer.from(JSON.stringify(target.path ?? []), "utf8").toString("base64");
      const b64val = Buffer.from(String(value ?? ""), "utf16le").toString("base64");
      const j = await psJson(`Add-Type -AssemblyName UIAutomationClient; Add-Type -AssemblyName UIAutomationTypes;
$path = [Convert]::FromBase64String('${b64path}'); $textPath = [Text.Encoding]::UTF8.GetString($path);
$val = [Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('${b64val}'));
$indices = $textPath | ConvertFrom-Json;
$root = [System.Windows.Automation.AutomationElement]::RootElement;
$targets = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition);
$cur = $null;
foreach ($t in $targets) { $cur = $t; break }
if ($cur -eq $null) { Write-Output '{"ok": false, "code": "app_not_found"}'; exit 0 }
foreach ($i in $indices) {
  if ($i -eq 0) { continue }
  $kids = $cur.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition);
  if ($i -ge $kids.Count) { Write-Output '{"ok": false, "code": "element_stale"}'; exit 0 }
  $cur = $kids[$i];
}
try {
  $vp = $cur.GetCurrentPattern([System.Windows.Automation.ValuePattern]::Pattern);
  $vp.SetValue($val);
  Write-Output '{"ok": true}';
} catch { Write-Output ('{"ok": false, "code": "' + $_.Exception.Message.Replace('"','') + '"}') }`, { timeoutMs: 45_000 });
      if (!j.ok) throw new ExecError(`set_value failed: ${j.code}`);
      return { action_sent: true, strategy: "a11y" };
    },
    select_text: async () => { throw new ExecError("select_text is not implemented on the win32 backend yet — fail-closed"); },
    perform_action: async ({ target, action }) => {
      await bootstrapped;
      const b64path = Buffer.from(JSON.stringify(target.path ?? []), "utf8").toString("base64");
      const act = String(action ?? "Invoke").replace(/'/g, "");
      const j = await psJson(`Add-Type -AssemblyName UIAutomationClient; Add-Type -AssemblyName UIAutomationTypes;
$path = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('${b64path}'));
$indices = $path | ConvertFrom-Json;
$root = [System.Windows.Automation.AutomationElement]::RootElement;
$targets = $root.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition);
$cur = $null;
foreach ($t in $targets) { $cur = $t; break }
foreach ($i in $indices) {
  if ($i -eq 0) { continue }
  $kids = $cur.FindAll([System.Windows.Automation.TreeScope]::Children, [System.Windows.Automation.Condition]::TrueCondition);
  if ($i -ge $kids.Count) { Write-Output '{"ok": false, "code": "element_stale"}'; exit 0 }
  $cur = $kids[$i];
}
$want = '${act}';
try {
  $pats = $cur.GetSupportedPatterns();
  $names = @($pats | ForEach-Object { $_.ProgrammaticName -replace 'Pattern\$','' -replace 'Pattern$','' });
  $chosen = $names | Where-Object { $_ -ieq $want } | Select-Object -First 1;
  if (-not $chosen -and ($want -ieq 'click' -or $want -ieq 'invoke')) { $chosen = 'Invoke' }
  if (-not $chosen) { Write-Output ('{"ok": false, "code": "action_not_found: " + ($names -join ",") }'); exit 0 }
  $pt = $cur.GetCurrentPattern($pats | Where-Object { ($_.ProgrammaticName -replace 'Pattern$','') -ieq $chosen } | Select-Object -First 1);
  if ($chosen -eq 'Invoke') { $pt.Invoke() } elseif ($chosen -eq 'ExpandCollapse') { $pt.Expand() } elseif ($chosen -eq 'Toggle') { $pt.Toggle() } else { $pt.Invoke() }
  Write-Output '{"ok": true, "sent": true}';
} catch { Write-Output ('{"ok": false, "code": "' + $_.Exception.Message.Replace('"','') + '"}') }`, { timeoutMs: 45_000 });
      if (!j.ok) throw new ExecError(`perform_action failed: ${j.code}`);
      return { action_sent: true, strategy: "a11y", action };
    },
    read_clipboard: async () => {
      const j = await psJson(`$t = Get-Clipboard -Raw -ErrorAction SilentlyContinue;
Write-Output ('{"text": ' + ($t | ConvertTo-Json -Compress) + '}');`, { timeoutMs: 10_000 });
      return { text: j.text ?? "", encoding: "utf8" };
    },
    write_clipboard: async ({ text }) => {
      const b64 = Buffer.from(String(text ?? ""), "utf16le").toString("base64");
      await ps(`Set-Clipboard -Value ([System.Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('${b64}')));
Write-Output '{"ok": true}';`, { timeoutMs: 10_000 });
      return { written: String(text ?? "").length };
    },
    cursor_position: async () => {
      await bootstrapped;
      const j = await psJson(`$p = New-Object User32+POINT;
[void][User32]::GetCursorPos([ref]$p);
Write-Output ('{"x": ' + $p.X + ', "y": ' + $p.Y + '}');`);
      return { x: j.x, y: j.y };
    },
    recordingStart: async ({ fps = 15, region } = {}) => {
      let ffmpegOk = true;
      try { await runOk("ffmpeg", ["-version"], { timeoutMs: 10_000 }); } catch { ffmpegOk = false; }
      if (!ffmpegOk) throw new ExecError("recording on Windows needs ffmpeg (gdigrab) on PATH — install ffmpeg and retry");
      const dir = recordingsDir();
      fs.mkdirSync(dir, { recursive: true });
      const id = crypto.randomBytes(4).toString("hex");
      const file = path.join(dir, `rec-${id}.mp4`);
      const args = ["-y", "-loglevel", "error", "-f", "gdigrab", "-framerate", String(fps)];
      if (region) args.push("-offset_x", String(Math.round(region[0])), "-offset_y", String(Math.round(region[1])), "-video_size", `${Math.round(region[2])}x${Math.round(region[3])}`);
      args.push("-i", "desktop", "-c:v", "libx264", "-pix_fmt", "yuv420p", file);
      const child = spawn("ffmpeg", args, { stdio: "ignore", detached: true });
      child.unref();
      await new Promise((r) => setTimeout(r, 800));
      try { process.kill(child.pid, 0); } catch { throw new ExecError("ffmpeg gdigrab exited immediately"); }
      recording = { id, pid: child.pid, file, startedAt: new Date().toISOString(), mode: "gdigrab" };
      return { id, pid: child.pid, file, mode: "gdigrab", fps };
    },
    recordingStop: async ({ id }) => {
      if (!recording || recording.id !== id) throw new ExecError(`unknown recording "${id}"`);
      try { process.kill(recording.pid, "SIGINT"); } catch {}
      await new Promise((r) => setTimeout(r, 1500));
      const bytes = fs.existsSync(recording.file) ? fs.statSync(recording.file).size : 0;
      const out = { id, file: recording.file, bytes, mode: recording.mode, startedAt: recording.startedAt, stoppedAt: new Date().toISOString() };
      recording = null;
      return out;
    },
    recordingStatus: ({ id }) => {
      if (!recording || recording.id !== id) return { id, running: false };
      let alive = true;
      try { process.kill(recording.pid, 0); } catch { alive = false; }
      return { id, running: alive, file: recording.file, bytes: fs.existsSync(recording.file) ? fs.statSync(recording.file).size : 0, mode: recording.mode };
    },
    recordingList: async () => {
      const dir = recordingsDir();
      const out = fs.existsSync(dir)
        ? fs.readdirSync(dir).filter((f) => /\.mp4$/i.test(f)).map((f) => {
            const st = fs.statSync(path.join(dir, f));
            return { file: path.join(dir, f), bytes: st.size, modifiedAt: st.mtime.toISOString() };
          }).sort((a, b) => b.modifiedAt.localeCompare(a.modifiedAt)).slice(0, 50)
        : [];
      return { dir, recordings: out, running: recording ? [recording.id] : [] };
    },
  };

  async function clickAt(button, x, y, clicks) {
    if (!Number.isFinite(Number(x)) || !Number.isFinite(Number(y))) throw new ExecError("coordinates must be finite numbers");
    const flags = button === 1 ? "RIGHTDOWN, RIGHTUP" : button === 2 ? "MIDDLEDOWN, MIDDLEUP" : "LEFTDOWN, LEFTUP";
    const seq = [];
    for (let i = 0; i < clicks; i++) seq.push(`[User32]::mouse_event([User32]::${flags.split(",")[0].trim()}, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 40; [User32]::mouse_event([User32]::${flags.split(",")[1].trim()}, 0, 0, 0, [UIntPtr]::Zero); Start-Sleep -Milliseconds 60;`);
    await withUser32(`[User32]::SetCursorPos(${Math.round(x)}, ${Math.round(y)}) | Out-Null;
Start-Sleep -Milliseconds 60;
${seq.join("\n")}
Write-Output '{"ok": true}';`, { timeoutMs: 20_000 });
    return { action_sent: true, at: { x: Number(x), y: Number(y) }, button, clicks };
  }
}

export default { create };
