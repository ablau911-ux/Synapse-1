#!/usr/bin/env python3
"""Synapse MCP daemon control panel - compact, anti-aliased Tkinter GUI.

Four status lamps in a row (Daemon / Server / MCP tools / Autostart): each a big
green/red LED with "Active"/"Not Active" above and the function name below.
Daemon and Autostart carry an on/off slider; Server + MCP tools are status-only.
A Safe Reclaim button starts only when the port is free; it never kills an
unverified listener.

Smoothness: the LED, slider and button are rendered as Pillow anti-aliased
images (4x supersample -> LANCZOS), cached per state, and swapped on change -
crisp like vectors, with zero per-frame redraw cost.

Resource use: polling is light - daemon pid comes from the port owner (no
process scan; see synapse_core.get_state), the MCP handshake is probed only
every ~9s and cached, and the loop ticks every 3s.

All daemon logic lives in synapse_core.SynapseService.
Run:  pythonw "%USERPROFILE%\\Synapse\\tools\\control-panel\\synapse_panel.py"
"""
from __future__ import annotations

import threading
import tkinter as tk

from PIL import Image, ImageDraw, ImageTk

from synapse_core import ENDPOINT, EXE, SynapseService


# --------------------------------------------------------------------------- #
#  Palette - soft light theme
# --------------------------------------------------------------------------- #
class C:
    BG       = "#eef1f8"
    CARD     = "#ffffff"
    TEXT     = "#2b3047"
    MUTED    = "#7e859c"
    INK      = "#243018"
    PURPLE   = "#c5b4ee"
    PURPLE_D = "#b3a0e6"
    GREY     = "#dfe3ef"
    GREY_D   = "#e9ebf3"
    BLUE     = "#aac4f2"
    BLUE_LED = "#4a86e8"
    LED_ON   = "#6cab46"
    LED_ON_E = "#4d7d31"
    LED_OFF  = "#d11f1f"
    LED_OFF_E = "#9e1414"
    ACT      = "#3f8f2e"
    INACT    = "#c42b2b"


_SS = 4  # supersample factor for anti-aliasing


def _hex_rgba(hex_str, a=255):
    h = hex_str.lstrip("#")
    return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16), a)


def _aa_disc(d, fill, outline, ow=2):
    """Anti-aliased filled circle of diameter d as a PhotoImage."""
    s = _SS
    img = Image.new("RGBA", (d * s, d * s), (0, 0, 0, 0))
    pad = 2 * s
    ImageDraw.Draw(img).ellipse(
        [pad, pad, d * s - pad, d * s - pad],
        fill=_hex_rgba(fill), outline=_hex_rgba(outline), width=ow * s)
    return ImageTk.PhotoImage(img.resize((d, d), Image.LANCZOS))


def _aa_round_rect(w, h, r, fill):
    """Anti-aliased rounded rectangle as a PhotoImage."""
    s = _SS
    img = Image.new("RGBA", (w * s, h * s), (0, 0, 0, 0))
    ImageDraw.Draw(img).rounded_rectangle(
        [0, 0, w * s - 1, h * s - 1], radius=r * s, fill=_hex_rgba(fill))
    return ImageTk.PhotoImage(img.resize((w, h), Image.LANCZOS))


# --------------------------------------------------------------------------- #
#  Widgets (cached anti-aliased images)
# --------------------------------------------------------------------------- #
class Led(tk.Label):
    """Large status lamp: green when active, red when not. Cached AA images."""
    SZ = 52

    def __init__(self, master, bg=C.CARD, **kw):
        super().__init__(master, bg=bg, **kw)
        self._img_on = _aa_disc(self.SZ, C.LED_ON, C.LED_ON_E)
        self._img_off = _aa_disc(self.SZ, C.LED_OFF, C.LED_OFF_E)
        self.configure(image=self._img_off)

    def set(self, on: bool):
        self.configure(image=self._img_on if on else self._img_off)


class ToggleSwitch(tk.Label):
    """Slider with a blue LED knob. command(new_value: bool). Cached AA images."""
    W, H = 56, 28

    def __init__(self, master, command, bg=C.CARD, **kw):
        super().__init__(master, bg=bg, cursor="hand2", **kw)
        self.command = command
        self._on = False
        self._enabled = True
        self._img_on = self._render(True)
        self._img_off = self._render(False)
        self.configure(image=self._img_off)
        self.bind("<Button-1>", lambda _: self._press())

    def _render(self, on: bool):
        s, W, H = _SS, self.W, self.H
        img = Image.new("RGBA", (W * s, H * s), (0, 0, 0, 0))
        d = ImageDraw.Draw(img)
        track = C.BLUE if on else C.GREY
        d.rounded_rectangle([2 * s, 5 * s, (W - 2) * s, (H - 5) * s],
                            radius=((H - 10) // 2) * s, fill=_hex_rgba(track))
        r = 11
        cy = H // 2
        cx = (W - 3 - r) if on else (3 + r)
        knob = C.BLUE_LED if on else "#ffffff"
        outline = C.BLUE_LED if on else C.MUTED
        d.ellipse([(cx - r) * s, (cy - r) * s, (cx + r) * s, (cy + r) * s],
                  fill=_hex_rgba(knob), outline=_hex_rgba(outline), width=1 * s)
        return ImageTk.PhotoImage(img.resize((W, H), Image.LANCZOS))

    def _press(self):
        if self._enabled and self.command:
            self.command(not self._on)

    def set(self, on: bool):
        self._on = on
        self.configure(image=self._img_on if on else self._img_off)

    def set_enabled(self, on: bool):
        self._enabled = on
        self.configure(cursor="hand2" if on else "arrow")


class RoundButton(tk.Canvas):
    """Rounded button: AA rounded-rect image background + crisp Tk text on top."""
    def __init__(self, master, text, color, color_hi, command, w=160, h=44,
                 fg=C.INK, sub=None, bg=C.BG, **kw):
        super().__init__(master, width=w, height=h, bg=bg, highlightthickness=0, **kw)
        self.command = command
        self._enabled = True
        self._img_n = _aa_round_rect(w, h, 16, color)
        self._img_h = _aa_round_rect(w, h, 16, color_hi)
        self._img_d = _aa_round_rect(w, h, 16, C.GREY_D)
        self._bg = self.create_image(0, 0, anchor="nw", image=self._img_n)
        ty = h // 2 if not sub else h // 2 - 8
        self.create_text(w // 2, ty, text=text, fill=fg, font=("Segoe UI Semibold", 12))
        if sub:
            self.create_text(w // 2, h // 2 + 11, text=sub, fill=fg, font=("Segoe UI", 8))
        self.bind("<Enter>", lambda _: self._hover(True))
        self.bind("<Leave>", lambda _: self._hover(False))
        self.bind("<Button-1>", lambda _: self._press())
        self.config(cursor="hand2")

    def _hover(self, on):
        if self._enabled:
            self.itemconfig(self._bg, image=self._img_h if on else self._img_n)

    def _press(self):
        if self._enabled and self.command:
            self.command()

    def set_enabled(self, on: bool):
        self._enabled = on
        self.itemconfig(self._bg, image=self._img_n if on else self._img_d)
        self.config(cursor="hand2" if on else "arrow")


class RoundedCard(tk.Canvas):
    """Card with anti-aliased rounded corners: a cached rounded-rect image
    background with a content frame (`self.body`) embedded on top. Call fit()
    after populating body to size the card to its content."""
    def __init__(self, master, width, radius=18, fill=C.CARD, bg=C.BG, **kw):
        super().__init__(master, width=width, height=10, bg=bg,
                         highlightthickness=0, **kw)
        self._cw = width          # NOTE: not self._w — tkinter reserves _w (widget path)
        self._radius = radius
        self._fill = fill
        self._img = None
        self._bg_id = self.create_image(0, 0, anchor="nw")
        self.body = tk.Frame(self, bg=fill)
        self._win = self.create_window(width // 2, 0, window=self.body, anchor="n")

    def fit(self, pad=14):
        self.update_idletasks()
        h = self.body.winfo_reqheight() + pad * 2
        self._img = _aa_round_rect(self._cw, h, self._radius, self._fill)
        self.itemconfig(self._bg_id, image=self._img)
        self.configure(height=h)
        self.coords(self._win, self._cw // 2, pad)


# --------------------------------------------------------------------------- #
#  App
# --------------------------------------------------------------------------- #
class App(tk.Tk):
    POLL_MS = 3000        # status poll cadence
    MCP_EVERY = 3         # probe the MCP handshake every Nth poll (~9s)

    def __init__(self):
        super().__init__()
        self.svc = SynapseService()
        self._busy = False
        self._closing = False
        self._ticks = 0
        self._mcp_ready = False
        self._mcp_n = -1

        self.title("Synapse Control")
        self.configure(bg=C.BG)
        self.geometry("520x380")
        self.minsize(520, 380)
        self.resizable(False, False)
        self.protocol("WM_DELETE_WINDOW", self._on_close)
        try:
            if EXE.exists():
                self.iconbitmap(default=str(EXE))
        except Exception:
            pass

        self._build()
        self.refresh(probe_mcp=True)
        self.after(self.POLL_MS, self._tick)

    # --- layout ----------------------------------------------------------- #
    def _build(self):
        header = tk.Frame(self, bg=C.BG)
        header.pack(fill="x", pady=(12, 4), padx=18)
        tk.Label(header, text="SYNAPSE", bg=C.BG, fg=C.TEXT,
                 font=("Segoe UI", 17, "bold")).pack(side="left")
        tk.Label(header, text="  MCP control", bg=C.BG, fg=C.MUTED,
                 font=("Segoe UI", 11)).pack(side="left", anchor="s", pady=(0, 3))
        self.dot = tk.Canvas(header, width=14, height=14, bg=C.BG, highlightthickness=0)
        self.dot.pack(side="right", pady=(6, 0))
        self._dot = self.dot.create_oval(2, 2, 12, 12, fill=C.GREY, outline="")

        # rounded status card holding the 4 lamps
        card = RoundedCard(self, width=484, radius=18)
        card.pack(pady=8, padx=18)
        strip = tk.Frame(card.body, bg=C.CARD)
        strip.pack(fill="x", padx=10, pady=(12, 4))
        self.st_daemon, self.led_daemon, self.sw_daemon = self._cell(
            strip, 0, "Daemon", self.on_toggle_daemon)
        self.st_server, self.led_server, _ = self._cell(strip, 1, "Server", None)
        self.st_tools, self.led_tools, _ = self._cell(strip, 2, "MCP tools", None)
        self.st_auto, self.led_auto, self.sw_auto = self._cell(
            strip, 3, "Autostart", self.on_toggle_autostart)
        self.lbl_endpoint = tk.Label(card.body, text=ENDPOINT, bg=C.CARD, fg="#3a6fd0",
                                     font=("Consolas", 9), anchor="center")
        self.lbl_endpoint.pack(fill="x", pady=(2, 10))
        card.fit()

        # half-width safe-reclaim, centered
        row = tk.Frame(self, bg=C.BG)
        row.pack(fill="x", pady=(2, 0), padx=18)
        self.b_reclaim = RoundButton(
            row, "Safe Reclaim", C.PURPLE, C.PURPLE_D, self.on_reclaim,
            w=240, h=46, sub="never kills unverified owners")
        self.b_reclaim.pack()

        self.status = tk.Label(self, text="ready", bg=C.BG, fg=C.MUTED,
                               font=("Consolas", 9), anchor="w", wraplength=484,
                               justify="left")
        self.status.pack(fill="x", side="bottom", pady=(8, 10), padx=18)

    def _cell(self, master, col, name, slider_cmd):
        c = tk.Frame(master, bg=C.CARD)
        c.grid(row=0, column=col, sticky="nsew", padx=4)
        master.columnconfigure(col, weight=1)
        st = tk.Label(c, text="", bg=C.CARD, fg=C.MUTED, font=("Segoe UI", 10, "bold"))
        st.pack()
        led = Led(c)
        led.pack(pady=(3, 4))
        tk.Label(c, text=name, bg=C.CARD, fg=C.TEXT,
                 font=("Segoe UI", 10, "bold")).pack()
        sw = None
        if slider_cmd is not None:
            sw = ToggleSwitch(c, slider_cmd, bg=C.CARD)
            sw.pack(pady=(6, 0))
        else:
            tk.Frame(c, bg=C.CARD, height=ToggleSwitch.H + 6).pack()  # align heights
        return st, led, sw

    # --- state rendering -------------------------------------------------- #
    def _lamp(self, st, led, on):
        led.set(on)
        st.config(text=("Active" if on else "Not Active"),
                  fg=(C.ACT if on else C.INACT))

    def refresh(self, probe_mcp: bool = False):
        s = self.svc.get_state(check_mcp=probe_mcp)
        if probe_mcp:
            self._mcp_ready = s["mcp_ready"]
            self._mcp_n = s["tool_count"]

        active = s["bound"] and s["is_synapse"]
        self._lamp(self.st_daemon, self.led_daemon, s["is_synapse"])
        self._lamp(self.st_server, self.led_server, s["bound"])
        self._lamp(self.st_tools, self.led_tools, self._mcp_ready)
        self._lamp(self.st_auto, self.led_auto, s["autostart"])

        dotc = C.LED_ON if self._mcp_ready else ("#e0af68" if s["bound"] else C.LED_OFF)
        self.dot.itemconfig(self._dot, fill=dotc)

        self.sw_daemon.set(active)
        self.sw_auto.set(s["autostart"])

        detail = []
        if s["syn_pids"]:
            detail.append("pid " + ", ".join(map(str, s["syn_pids"])))
        if self._mcp_ready and self._mcp_n > 0:
            detail.append(f"{self._mcp_n} tools")
        if s["proxied"]:
            detail.append("proxy " + ", ".join(
                f"{n}({p})" for p, n in s["owners"]) +
                f" -> daemon pid {', '.join(map(str, s['syn_pids']))}")
        elif s["bound"] and not s["is_synapse"]:
            detail.append("port held by: " + ", ".join(f"{n}({p})" for p, n in s["owners"]))
        if s["bound"]:
            detail.append("Safe Reclaim blocked; no port owner will be killed")
        if not s["exe_exists"]:
            detail.append("WARNING binary not found")
        if not self._busy:
            self.status.config(text=("  ".join(detail)) or "ready", fg=C.MUTED)

        self.sw_daemon.set_enabled(not self._busy)
        self.sw_auto.set_enabled(not self._busy)
        self.b_reclaim.set_enabled(not self._busy and not s["bound"])

    def _tick(self):
        self._ticks += 1
        if not self._busy:
            self.refresh(probe_mcp=(self._ticks % self.MCP_EVERY == 0))
        self.after(self.POLL_MS, self._tick)

    # --- threaded action runner ------------------------------------------ #
    _WORK, _OK, _ERR = "#9a7b16", "#2f7d32", "#b3261e"

    def _run(self, fn, verb):
        if self._busy:
            return
        self._busy = True
        self.status.config(text=f"{verb}...", fg=self._WORK)
        self.refresh()

        def worker():
            try:
                msg, col = fn(), self._OK
            except Exception as e:
                msg, col = f"error: {e}", self._ERR
            if self._closing:
                return
            try:
                self.after(0, lambda: self._done(msg, col))
            except tk.TclError:
                pass

        threading.Thread(target=worker, daemon=True).start()

    def _done(self, msg, col):
        self._busy = False
        self.status.config(text=msg, fg=col)
        self.refresh(probe_mcp=True)

    def _on_close(self):
        self._closing = True
        self.destroy()

    # --- handlers --------------------------------------------------------- #
    def on_reclaim(self):
        self._run(self.svc.force_reclaim, "checking safe reclaim")

    def on_toggle_daemon(self, turn_on: bool):
        if turn_on:
            self._run(self.svc.start, "starting daemon")
        else:
            self._run(self.svc.stop, "stopping daemon")

    def on_toggle_autostart(self, turn_on: bool):
        if turn_on:
            self._run(self.svc.enable_autostart, "enabling autostart")
        else:
            self._run(self.svc.disable_autostart, "disabling autostart")


if __name__ == "__main__":
    App().mainloop()
