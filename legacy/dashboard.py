"""One desktop control centre for the RGB engine and installed Fan Control."""

import queue
import threading
import tkinter as tk
from tkinter import colorchooser, filedialog, messagebox, simpledialog, ttk

from fan_control import FanControlBridge


class Dashboard:
    def __init__(self, app, settings_dir, atomic_write):
        self.app = app
        self.atomic_write = atomic_write
        self.fans = FanControlBridge(settings_dir)
        self.commands = queue.Queue()
        self.root = tk.Tk()
        self.root.title("Cooling & Lighting")
        self.root.geometry("960x800")
        self.root.minsize(860, 760)
        self.root.configure(bg="#111827")
        self.root.protocol("WM_DELETE_WINDOW", self.root.withdraw)
        self.root.withdraw()
        style = ttk.Style(self.root)
        style.theme_use("clam")
        style.configure("TFrame", background="#111827")
        style.configure("TLabel", background="#111827", foreground="#e5e7eb", font=("Segoe UI", 10))
        style.configure("Title.TLabel", font=("Segoe UI", 24, "bold"), foreground="#f9fafb")
        style.configure("Heading.TLabel", font=("Segoe UI", 14, "bold"))
        style.configure("TButton", font=("Segoe UI", 10), padding=(12, 7))
        style.configure("Treeview", background="#1f2937", fieldbackground="#1f2937", foreground="#e5e7eb", rowheight=32)
        style.map("Treeview", background=[("selected", "#2563eb")])
        frame = ttk.Frame(self.root, padding=24)
        frame.pack(fill="both", expand=True)
        ttk.Label(frame, text="Cooling & Lighting", style="Title.TLabel").pack(anchor="w")
        ttk.Label(frame, text="Your fan profiles and RGB controls, together.").pack(anchor="w", pady=(3, 18))
        ttk.Label(frame, text="Lighting", style="Heading.TLabel").pack(anchor="w")
        self.rgb_status = tk.StringVar()
        ttk.Label(frame, textvariable=self.rgb_status).pack(anchor="w", pady=(6, 8))
        actions = ttk.Frame(frame)
        actions.pack(fill="x")
        self.toggle = ttk.Button(actions, text="Turn lights off", command=lambda: app._toggle(None, None))
        self.toggle.pack(side="left", padx=(0, 8))
        ttk.Button(actions, text="Custom colour…", command=self.custom_color).pack(side="left", padx=(0, 8))
        ttk.Button(actions, text="Reconnect RGB", command=lambda: app._reconnect(None, None)).pack(side="left")
        swatches = ttk.Frame(frame)
        swatches.pack(fill="x", pady=(10, 20))
        from main import PRESET_COLORS
        for name, rgb in PRESET_COLORS:
            hex_color = "#%02x%02x%02x" % rgb
            tk.Button(swatches, text=name, bg=hex_color, activebackground=hex_color,
                      fg="white" if sum(rgb) < 400 else "black", relief="flat", padx=8,
                      pady=7, command=lambda rgb=rgb: app._apply_color(rgb)).pack(side="left", padx=(0, 5))
        ttk.Separator(frame).pack(fill="x", pady=(0, 16))
        ttk.Label(frame, text="Cooling", style="Heading.TLabel").pack(anchor="w")
        self.fan_status = tk.StringVar(value="Checking Fan Control…")
        ttk.Label(frame, textvariable=self.fan_status).pack(anchor="w", pady=(5, 8))
        profile_bar = ttk.Frame(frame)
        profile_bar.pack(fill="x", pady=(0, 10))
        self.selected = tk.StringVar()
        self.profiles_combo = ttk.Combobox(profile_bar, textvariable=self.selected, state="readonly", width=37)
        self.profiles_combo.pack(side="left", padx=(0, 8))
        self.profiles_combo.bind("<<ComboboxSelected>>", lambda event: self.show_profile())
        ttk.Button(profile_bar, text="Apply profile", command=self.apply_profile).pack(side="left", padx=(0, 8))
        ttk.Button(profile_bar, text="Refresh", command=self.refresh_profiles).pack(side="left")
        self.table = ttk.Treeview(frame, columns=("control", "mode", "paired"), show="headings", height=5)
        for key, label, width in (("control", "Fan control", 285), ("mode", "Saved cooling setting", 315), ("paired", "RPM sensor", 105)):
            self.table.heading(key, text=label)
            self.table.column(key, width=width)
        self.table.pack(fill="both", expand=True)
        ttk.Label(frame, text="Saved profile settings above; live RPM, temperatures and curve tuning are in the fan editor.", wraplength=810).pack(anchor="w", pady=8)
        fan_actions = ttk.Frame(frame)
        fan_actions.pack(fill="x")
        ttk.Button(fan_actions, text="Fan curves & live sensors…", command=lambda: self.background(self.fans.open_editor, "Opened Fan Control editor")).pack(side="left", padx=(0, 8))
        ttk.Button(fan_actions, text="Save profile copy…", command=self.copy_profile).pack(side="left", padx=(0, 8))
        ttk.Button(fan_actions, text="Import profile…", command=self.import_profile).pack(side="left")
        self.notice = tk.StringVar(value="Closing this window keeps cooling and lighting running in the tray.")
        ttk.Label(frame, textvariable=self.notice, wraplength=810).pack(anchor="w", pady=(16, 0))
        self.profiles = {}
        self.refresh_profiles()
        self.root.after(100, self.poll)
        self.background(self.fans.ensure_running, "Fan Control is running")

    def show(self):
        self.root.deiconify()
        self.root.lift()
        self.root.focus_force()
        self.refresh_profiles()

    def custom_color(self):
        result = colorchooser.askcolor(color="#%02x%02x%02x" % self.app.color,
                                      title="Choose lighting colour", parent=self.root)
        if result[0] is not None:
            self.app._apply_color(tuple(round(c) for c in result[0]))

    def background(self, action, success):
        def work():
            try:
                action()
                self.commands.put(("notice", success))
            except Exception as exc:
                self.commands.put(("error", str(exc)))
        threading.Thread(target=work, daemon=True).start()

    def refresh_profiles(self):
        old = self.selected.get()
        self.profiles = {}
        for path in self.fans.profiles():
            label = f"{path.stem} ({'saved copy' if path.parent == self.fans.profile_dir else 'Fan Control'})"
            self.profiles[label] = path
        self.profiles_combo["values"] = list(self.profiles)
        self.selected.set(old if old in self.profiles else next(iter(self.profiles), ""))
        self.show_profile()

    def show_profile(self):
        self.table.delete(*self.table.get_children())
        path = self.profiles.get(self.selected.get())
        if not path:
            return
        try:
            data = self.fans.read_profile(path)
            for control in data["FanControl"]["Controls"]:
                if control.get("IsHidden"):
                    continue
                self.table.insert("", "end", values=(control.get("NickName", control.get("Identifier", "Fan")),
                    self.fans.control_summary(control), "Paired" if control.get("PairedFanSensor") else "Unpaired"))
        except (OSError, ValueError) as exc:
            self.notice.set(str(exc))

    def apply_profile(self):
        path = self.profiles.get(self.selected.get())
        if path:
            self.notice.set("Sending profile to Fan Control; approve Windows elevation if requested.")
            self.background(lambda: self.fans.apply(path),
                            f"Profile command sent: {path.stem}. Check the fan editor for live values.")

    def copy_profile(self):
        path = self.profiles.get(self.selected.get())
        if path:
            self.save_copy(self.fans.read_profile(path))

    def import_profile(self):
        name = filedialog.askopenfilename(parent=self.root, title="Import Fan Control profile", filetypes=[("Fan Control profiles", "*.json")])
        if name:
            try:
                self.save_copy(self.fans.read_profile(name))
            except (OSError, ValueError) as exc:
                messagebox.showerror("Profile import", str(exc), parent=self.root)

    def save_copy(self, data):
        name = simpledialog.askstring("Save cooling profile", "Profile name:", parent=self.root)
        if name is None:
            return
        try:
            path = self.fans.save_profile(name, data, self.atomic_write)
            self.refresh_profiles()
            self.selected.set(f"{path.stem} (saved copy)")
            self.show_profile()
            self.notice.set(f"Saved profile copy: {name}. Fan calibration and curves are preserved.")
        except (OSError, ValueError) as exc:
            messagebox.showerror("Save cooling profile", str(exc), parent=self.root)

    def poll(self):
        show_request = self.fans.settings_dir / "show.request"
        if show_request.exists():
            try:
                show_request.unlink()
                self.show()
            except OSError:
                pass
        try:
            while True:
                kind, value = self.commands.get_nowait()
                if kind == "show":
                    self.show()
                elif kind == "color":
                    self.custom_color()
                elif kind == "quit":
                    self.root.quit()
                    return
                else:
                    self.notice.set(value)
        except queue.Empty:
            pass
        color = "#%02x%02x%02x" % self.app.color
        state = "On" if self.app.on else "Off"
        status = "applied" if self.app._applied else "waiting for RGB devices"
        self.rgb_status.set(f"{state}  ·  {color.upper()}  ·  {status}" + ("  ·  Settings save failed" if not self.app._save_ok else ""))
        self.toggle.configure(text="Turn lights off" if self.app.on else "Turn lights on")
        self.fan_status.set("Fan Control is running" if self.fans.running else self.fans.error or "Fan Control is not running — open the fan editor to start it")
        self.root.after(200, self.poll)

    def monitor(self):
        if not self.app._stopping.is_set():
            self.fans.refresh()

    def run(self):
        try:
            self.root.mainloop()
        finally:
            self.root.destroy()
