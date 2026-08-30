# Microsoft ConPTY redistributable

This directory vendors Microsoft.Windows.Console.ConPTY `1.24.260710001` from
the Windows Terminal `v1.24.11911.0` release. The Windows build embeds these
files in its single executable, then atomically materializes this exact layout
and `LICENSE.txt` in a versioned per-user local runtime cache before first use.

Source package:
https://github.com/microsoft/terminal/releases/download/v1.24.11911.0/Microsoft.Windows.Console.ConPTY.1.24.260710001.nupkg

The source package SHA-256 is
`9382ad7becb7e4d84e300578d8e4f4df28f43d979d9055d978c42913c47e0e9d`.

| Vendored path | Package path | SHA-256 |
| --- | --- | --- |
| `conpty.dll` | `runtimes/win-x64/native/conpty.dll` | `39fba2713e2495117b1591ae8c32a3b904bea7aa66069cf7815e2844c76d75d8` |
| `x64/OpenConsole.exe` | `build/native/runtimes/x64/OpenConsole.exe` | `b7fd936c2668b87b9ecf7b3366dc6568afc1c6f981874cba3e955a1c35cf8160` |
| `arm64/OpenConsole.exe` | `build/native/runtimes/arm64/OpenConsole.exe` | `ed7622fd0d3bedc9ab9f122f5e58edf0def9e7999224f52dd395ba9f54edbe09` |

Microsoft's x64 package intentionally deploys both OpenConsole hosts. An x64
application can run on either an x64 system or under emulation on ARM64, and
`conpty.dll` selects the native host. Keep all three files on the same version;
do not update or redistribute them independently.

The package is copyright Microsoft Corporation and licensed under the MIT
License. See `LICENSE.txt` in this directory.
