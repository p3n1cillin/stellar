# Third-party software

Stellar's own code is MIT-licensed (see `LICENSE`). The release download also contains:

| Component | Used for | License |
|---|---|---|
| [LibreHardwareMonitorLib](https://github.com/LibreHardwareMonitor/LibreHardwareMonitor) | sensors and fan control, in `hw\` | MPL-2.0 (source at the link) |
| HidSharp, BlackSharp.Core, DiskInfoToolkit, RAMSPDToolkit-NDD, Mono.Posix.NETStandard | LibreHardwareMonitor dependencies, in `hw\` | Apache-2.0 / MIT (see each package) |
| .NET 8 runtime | runs the hardware host, in `hw\` | MIT |
| egui / eframe, tray-icon, serde, chrono, sysinfo, image, winresource, windows-sys and other Rust crates | the Stellar app | MIT and/or Apache-2.0 (see `Cargo.lock` and each crate) |

These separate programs are **not** included. Stellar can install them with winget, with your permission:

| Program | Used for | License |
|---|---|---|
| [PawnIO](https://pawnio.eu) | kernel driver LibreHardwareMonitor uses to reach motherboard fan headers and CPU sensors | see pawnio.eu |
| [OpenRGB](https://openrgb.org) | talks to RGB devices; Stellar connects to its local SDK server | GPL-2.0 |
