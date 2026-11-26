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
4. It shows a 6-digit **pairing code** like `482 913`. Leave the window open.

On the **viewer**:

1. Open **View another PC** (or type `gamennetto view`).
2. It finds the host on the network. If several are found, pick one.
3. **Only the first time** these two PCs connect, approve the connection
   (see below). After that it connects straight away.

The picture appears in a window. Its title shows the frames per second, the
delay (from the host's screen to yours) and the data rate.

### Approving a PC (only the first time)

The viewer asks: *"Type the 6-digit code shown on the host, or press Enter to
approve it there instead"*. Either way works:

- **Type the code** shown in the host's window. Nothing else to do.
- **Or press Enter** to compare a number. The viewer shows a number, and the
  host's window asks *"LEGION wants to connect... Allow? [y/N]"* with the same
  number. If both numbers match, type `y` on the host, then confirm on the
  viewer. If they don't match, say no: something is wrong.

From then on, the two PCs **remember each other** and connect without any
code. A PC that wasn't approved can't connect, not even from your own network.
The connection is always encrypted.

- After 3 wrong codes the host makes a new code and refuses codes for 30
  seconds, so guessing it doesn't work.
- `gamennetto devices` lists the PCs this PC has approved, and
  `gamennetto devices remove NAME` forgets one (it must be approved again).
- Both PCs must have Gamennetto 0.2 or newer: older versions can't connect to
  newer ones.

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
| **Ctrl+Alt+S** | Picture size: **fit** (as big as possible, whole picture; the default), **fill** (whole window, edges trimmed) or **actual size** (pixel for pixel). Start with one using `gamennetto view --scale fill`. |
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
| "wrong code" | Type the code shown in the host's window *now*: it changes each time the host starts, and after 3 wrong tries. |
| "not allowed" | The host said no, or nobody answered there within 60 s. Try again and type `y` on the host. |
| Connection fails right away | Both PCs need the same Gamennetto version: run `gamennetto update` on both. |
| A PC should no longer connect | On the host: `gamennetto devices remove NAME`. |
| Black bars beside or above the picture | The two screens have different shapes (e.g. 16:10 and 16:9). Press F11 for full screen, and **Ctrl+Alt+S** for *fill* to cover the screen without stretching (a little of the edges is trimmed). In extend mode the extra screen already has your screen's shape. |
| Picture stutters or delay is high | Use **Fastest** quality, move closer to the router, or use 5 GHz Wi-Fi or a cable. |
| "no hardware encoder" or sharing won't start | Run `gamennetto doctor` and check "What Gamennetto would use". Sharing needs a GPU with a video encoder for now. |
| The extra screen stayed on | `gamennetto reset-displays`, or Win+P > PC screen only. |
| `gamennetto` is not recognized | Open a new terminal after installing (PATH changes need it), or use the **Gamennetto terminal** entry. |

`gamennetto --help` lists every command, and `gamennetto host --help` (or any
command followed by `--help`) lists its options.
