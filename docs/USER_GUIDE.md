# Gamennetto user guide

Gamennetto shows one PC's screen on another PC over your Wi-Fi or LAN:

- **Mirror**: the other PC shows a copy of your screen.
- **Extend**: the other PC becomes an extra screen. You drag windows onto it.

The PC that shares its screen is the **host**. The PC that watches is the
**viewer**. Both must be on the same network (same Wi-Fi or router).

## 1. Install (on both PCs)

1. Download `Gamennetto-X.Y.Z-setup.exe` from
   https://github.com/wilder00/gamennetto/releases/latest
2. Run it. It doesn't need administrator rights.
   - Windows may say "Windows protected your PC" (the installer isn't signed
     with a Windows certificate yet): click **More info**, then **Run anyway**.
   - Keep "Add gamennetto to PATH" ticked, so it works from any terminal.

You get these Start menu entries:

| Start menu entry | What it does |
| --- | --- |
| **Share this screen** | Host, mirror mode, view only. |
| **Use as extra screen (extend)** | Host, extend mode, view only. Needs the driver (step 4). |
| **View another PC** | Viewer: finds hosts and asks for the pairing code. |
| **Gamennetto terminal** | A terminal where you can type `gamennetto` commands. |

Each one opens a console window: that window *is* Gamennetto. Closing it stops
it. To let the viewer use your mouse and keyboard, start the host from a
terminal with `--allow-control` (see step 3).

The first time each PC uses the network, **Windows Firewall** asks whether to
allow Gamennetto. Choose **Private networks** and click **Allow**.

## 2. Mirror your screen

On the **host** (the PC whose screen you want to show):

1. Open **Share this screen** (or type `gamennetto host`).
2. If you have several monitors, pick one.
3. Pick a quality. Each choice is explained on screen:
   - **Fastest**: lowest delay, softer image (good for games and drawing).
   - **Balanced**: the default, good for most things.
   - **Sharpest**: crisp text, needs a strong connection (5 GHz Wi-Fi or cable).
4. It shows a **pairing code** like `ABCD-EF12-3456`. Leave the window open.

On the **viewer**:

1. Open **View another PC** (or type `gamennetto view`).
2. It finds the host on the network. If several are found, pick one.
3. Type the pairing code shown on the host.

The picture appears in a window. Its title shows the frames per second, the
delay (from the host's screen to yours) and the data rate.

The pairing code changes each time the host starts. Only a viewer that knows
it can connect, and the connection is encrypted.

## 3. Control the host from the viewer

Start the host from a terminal (the **Gamennetto terminal** entry, or any
PowerShell window) with:

```
gamennetto host --allow-control
```

In the viewer window, it works like a virtual machine window:

| Key / action | What it does |
| --- | --- |
| **Click the picture** | Take control: mouse and keyboard go to the host, including Alt+Tab, the Windows key and Alt+F4. The mouse stays inside the window. |
| **Ctrl+Alt** (press and release just those two) | Give your mouse and keyboard back to your own PC. |
| Switching to another window | Also gives them back. |
| **F11** | Full screen on/off. Never sent to the host. |
| **Ctrl+Alt+Q** | Close the viewer. |

The title bar says **CONTROLLING** while you control the host.
Ctrl+Alt+Del always stays on your own PC: Windows doesn't allow programs to
send it. Windows running as administrator on the host (and its UAC prompts)
can't be controlled unless the host runs Gamennetto as administrator too.

## 4. Use the other PC as an extra screen (extend)

Once, on the **host**, install the virtual monitor driver:

```
gamennetto display install
```

Windows asks for administrator permission and may ask whether to trust
"SignPath Foundation" (who signs the driver): accept both. The driver is the
open-source Virtual Display Driver (see THIRD_PARTY.md).

Then, each time:

1. On the host: **Use as extra screen (extend)**, or
   `gamennetto host --extend --allow-control`.
2. On the viewer: **View another PC**, as in step 2.

When the viewer connects, a new screen appears to the **right** of the host's
screens, sized to the viewer's screen, and the viewer goes full screen. Drag
windows to the right edge of the host's screen to move them onto it.
To use a fixed size instead: `gamennetto host --extend --size 1920x1080`.

The extra screen turns off by itself **10 seconds after the viewer leaves**,
or when you close the host window, and your screens go back to how they were.

If it ever stays on by mistake (for example after a crash):

```
gamennetto reset-displays
```

or press **Win+P** and choose **PC screen only** (then **Extend** again if you
had other monitors). Restarting Windows also turns it off.

Mirror mode doesn't need the driver at all.

## 5. Update

```
gamennetto update          # installs the newest version
gamennetto update --check  # only tells you if there is one
```

When a newer version exists, `host` and `view` mention it (at most once a
day). The update is checked against Gamennetto's signing key before it
installs, and it installs without asking for administrator rights.

## 6. Uninstall

Windows **Settings > Apps > Installed apps > Gamennetto > Uninstall**. If the
virtual monitor driver is installed, Windows asks for administrator permission
to remove it too. To remove only the driver: `gamennetto display uninstall`.

## Troubleshooting

| Problem | What to do |
| --- | --- |
| The viewer finds no host | Check both PCs are on the same Wi-Fi and the host window is open. Allow Gamennetto in Windows Firewall (Private networks). Or give the address shown by the host: `gamennetto view 192.168.1.20`. |
| A VPN is on (ProtonVPN, NordVPN...) | Gamennetto uses your Wi-Fi/Ethernet address, not the VPN's, but the VPN may block local traffic: turn on its "Allow LAN connections" option, or turn it off while sharing. |
| The host says port 48100 was busy | Nothing to do: it picked another port and viewers still find it. If you type the address by hand, include the port it shows, e.g. `gamennetto view 192.168.1.20:48101`. |
| "the host is busy" | Another viewer is connected. Only one viewer at a time for now. |
| Connection fails right away | Check the pairing code; it changes each time the host starts. |
| Picture stutters or delay is high | Use **Fastest** quality, move closer to the router, or use 5 GHz Wi-Fi or a cable. |
| "no hardware encoder" or sharing won't start | Run `gamennetto doctor` and check "What Gamennetto would use". Sharing needs a GPU with a video encoder for now. |
| The extra screen stayed on | `gamennetto reset-displays`, or Win+P > PC screen only. |
| `gamennetto` is not recognized | Open a new terminal after installing (PATH changes need it), or use the **Gamennetto terminal** entry. |

`gamennetto --help` lists every command, and `gamennetto host --help` (or any
command followed by `--help`) lists its options.
