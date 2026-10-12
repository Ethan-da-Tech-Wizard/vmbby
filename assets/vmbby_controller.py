#!/usr/bin/env python3
"""GTK4 controller for vmbby.

The Rust binary owns VM behavior. This helper gives the project a custom,
Wayland-friendly controller without pulling in Electron or a browser runtime.
"""

import math
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, GLib, Gdk, Gio, Gtk  # noqa: E402


ROOT = Path(__file__).resolve().parents[1]
BIN = shutil.which("vmbby") or str(ROOT / "target" / "debug" / "vmbby")


class Portal(Gtk.DrawingArea):
    def __init__(self):
        super().__init__()
        self.start = time.monotonic()
        self.set_content_width(840)
        self.set_content_height(310)
        self.set_draw_func(self.draw)
        self.add_tick_callback(self.tick)

    def tick(self, _widget, _clock):
        self.queue_draw()
        return GLib.SOURCE_CONTINUE

    def draw(self, _area, cr, width, height):
        t = time.monotonic() - self.start
        cx = width / 2
        cy = height / 2

        cr.set_source_rgb(0.014, 0.011, 0.026)
        cr.rectangle(0, 0, width, height)
        cr.fill()

        for i in range(58):
            phase = t * 1.45 + i * 0.34
            radius = 12 + i * 7.4 + math.sin(phase) * 16
            red = 0.50 + 0.45 * math.sin(phase)
            green = 0.38 + 0.48 * math.sin(phase + 2.1)
            blue = 0.58 + 0.38 * math.sin(phase + 4.2)
            alpha = max(0.035, 0.40 - i * 0.006)
            cr.set_source_rgba(red, green, blue, alpha)
            cr.set_line_width(1.6 + 1.4 * math.sin(phase * 0.5))
            wobble_x = math.sin(phase * 0.72) * 32
            wobble_y = math.cos(phase * 0.84) * 20
            cr.arc(cx + wobble_x, cy + wobble_y, radius, 0, math.tau)
            cr.stroke()

        for i in range(26):
            y = (i / 25) * height
            amp = 10 + 8 * math.sin(t + i)
            cr.set_source_rgba(0.15, 1.0, 0.88, 0.16)
            cr.set_line_width(1)
            cr.move_to(0, y)
            for x in range(0, width + 12, 12):
                cr.line_to(x, y + math.sin(x * 0.025 + t * 2.8 + i) * amp)
            cr.stroke()

        title = "vmbby"
        subtitle = "Arch host | Plasma/Hyprland Wayland | Ubuntu guest"
        cr.select_font_face("Sans", 0, 1)
        cr.set_font_size(68)
        ext = cr.text_extents(title)
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.55)
        cr.move_to(cx - ext.width / 2 + 5, cy + 16 + 5)
        cr.show_text(title)
        cr.set_source_rgb(1.0, 0.98, 0.48)
        cr.move_to(cx - ext.width / 2, cy + 16)
        cr.show_text(title)

        cr.select_font_face("Monospace", 0, 1)
        cr.set_font_size(15)
        ext = cr.text_extents(subtitle)
        cr.set_source_rgb(0.70, 1.0, 0.94)
        cr.move_to(cx - ext.width / 2, cy + 46)
        cr.show_text(subtitle)


class Controller(Adw.Application):
    def __init__(self):
        super().__init__(application_id="dev.ethan.vmbby")
        self.connect("activate", self.activate)

    def activate(self, app):
        win = Adw.ApplicationWindow(application=app)
        win.set_title("vmbby")
        win.set_default_size(940, 720)

        css = Gtk.CssProvider()
        css.load_from_data(
            b"""
            window { background: #080711; }
            .panel { padding: 16px; background: #090817; }
            .strip { padding: 12px; background: #15142a; border-radius: 8px; }
            .status { color: #10211e; background: #effffb; font-family: monospace; font-size: 13px; }
            .headline { color: #fff892; font-weight: 900; font-size: 24px; }
            .subtle { color: #b7fff2; }
            .pill { color: #080711; background: #b7fff2; border-radius: 999px; padding: 5px 10px; font-weight: 800; }
            button { border-radius: 8px; font-weight: 800; padding: 10px 14px; color: #080711; background: #fff36d; }
            button:hover { background: #34ffe0; }
            .hot { background: #ff4bd8; color: #100713; }
            .go { background: #31ff9b; color: #07130d; }
            .danger { background: #ff604f; color: #160504; }
            entry { color: #effffb; background: #111022; border-radius: 8px; padding: 9px; }
            """
        )
        Gtk.StyleContext.add_provider_for_display(
            Gdk.Display.get_default(),
            css,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION,
        )

        box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        box.add_css_class("panel")
        win.set_content(box)

        box.append(Portal())

        head = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
        head.add_css_class("strip")
        box.append(head)

        title_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
        title = Gtk.Label(label="Ubuntu cockpit")
        title.set_xalign(0)
        title.add_css_class("headline")
        subtitle = Gtk.Label(label="Real Ubuntu VM in one controlled Wayland window")
        subtitle.set_xalign(0)
        subtitle.add_css_class("subtle")
        title_box.append(title)
        title_box.append(subtitle)
        title_box.set_hexpand(True)
        head.append(title_box)

        self.state = Gtk.Label(label="READY")
        self.state.add_css_class("pill")
        head.append(self.state)

        iso_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        iso_row.add_css_class("strip")
        box.append(iso_row)
        self.iso_entry = Gtk.Entry()
        self.iso_entry.set_placeholder_text("/path/to/ubuntu-24.04.iso")
        self.iso_entry.set_hexpand(True)
        iso_row.append(self.iso_entry)
        browse = Gtk.Button(label="Browse ISO")
        browse.connect("clicked", self.browse_iso, win)
        iso_row.append(browse)
        install = Gtk.Button(label="Install Ubuntu")
        install.add_css_class("hot")
        install.connect("clicked", self.install_clicked)
        iso_row.append(install)

        buttons = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        buttons.add_css_class("strip")
        box.append(buttons)
        for label, command, css_class in [
            ("Doctor", ["doctor"], None),
            ("Init Disk", ["init"], None),
            ("Run Ubuntu", ["run"], "go"),
            ("Status", ["status"], None),
            ("Logs", ["logs"], None),
            ("Explain", ["explain"], None),
            ("Stop", ["stop"], "danger"),
        ]:
            btn = Gtk.Button(label=label)
            if css_class:
                btn.add_css_class(css_class)
            btn.connect("clicked", self.command_clicked, command)
            buttons.append(btn)

        self.output = Gtk.TextView()
        self.output.set_editable(False)
        self.output.set_monospace(True)
        self.output.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
        self.output.add_css_class("status")
        scroller = Gtk.ScrolledWindow()
        scroller.set_vexpand(True)
        scroller.set_min_content_height(220)
        scroller.set_child(self.output)
        box.append(scroller)

        self.write(
            "vmbby controller ready\n"
            "1. Click Doctor.\n"
            "2. Click Init Disk.\n"
            "3. Pick the Ubuntu ISO and click Install Ubuntu.\n"
            "4. After Ubuntu is installed, click Run Ubuntu.\n\n"
        )
        win.present()

    def command_clicked(self, _button, command):
        self.run_command(command)

    def install_clicked(self, _button):
        iso = self.iso_entry.get_text().strip()
        if not iso:
            self.write("\nPick an Ubuntu ISO first. Example: ubuntu-24.04-desktop-amd64.iso\n")
            return
        self.run_command(["install", iso])

    def browse_iso(self, _button, win):
        dialog = Gtk.FileDialog()
        dialog.set_title("Choose Ubuntu ISO")
        iso_filter = Gtk.FileFilter()
        iso_filter.set_name("ISO images")
        iso_filter.add_pattern("*.iso")
        filters = Gio.ListStore.new(Gtk.FileFilter)
        filters.append(iso_filter)
        dialog.set_filters(filters)
        dialog.open(win, None, self.iso_chosen)

    def iso_chosen(self, dialog, result):
        try:
            file = dialog.open_finish(result)
            if file:
                self.iso_entry.set_text(file.get_path() or "")
        except GLib.Error as exc:
            message = str(exc).lower()
            if "dismiss" not in message and "cancel" not in message:
                self.write(f"ISO picker failed: {exc}\n")

    def run_command(self, command):
        self.write(f"\n$ {BIN} {' '.join(command)}\n")
        self.state.set_text("RUNNING")
        thread = threading.Thread(target=self.command_worker, args=(command,), daemon=True)
        thread.start()

    def command_worker(self, command):
        try:
            proc = subprocess.run([BIN, *command], cwd=ROOT, text=True, capture_output=True, check=False)
            text = ""
            if proc.stdout:
                text += proc.stdout
            if proc.stderr:
                text += proc.stderr
            if proc.returncode != 0:
                text += f"[exit code {proc.returncode}]\n"
            GLib.idle_add(self.finish_command, text or "done\n")
        except Exception as exc:
            GLib.idle_add(self.finish_command, f"failed: {exc}\n")

    def finish_command(self, text):
        self.write(text)
        self.state.set_text("READY")
        return GLib.SOURCE_REMOVE

    def write(self, text):
        buf = self.output.get_buffer()
        end = buf.get_end_iter()
        buf.insert(end, text)
        mark = buf.create_mark(None, buf.get_end_iter(), False)
        self.output.scroll_to_mark(mark, 0.0, True, 0.0, 1.0)


if __name__ == "__main__":
    raise SystemExit(Controller().run(sys.argv))
