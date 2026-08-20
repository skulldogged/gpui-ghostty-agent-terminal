# Separate desktop presence from the Resident Core

The per-user Resident Core owns Spaces and Terminal Sessions, while an independently restartable Desktop Shell owns GPUI windows and optional tray or status-item integration. This two-process boundary keeps terminal survival independent of presentation crashes and platform event loops without introducing a third dedicated tray process; closing or quitting presentation therefore never stops the Resident Core.
