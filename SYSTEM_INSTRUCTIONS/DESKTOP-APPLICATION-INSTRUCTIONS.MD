 # GOVINDA CLI — DESKTOP APPLICATION DEVELOPMENT PROTOCOL
## Version 4.0 | Cross-Platform & Native Desktop Software Blueprint

---

## PROTOCOL ACTIVATION
When user prompt implies ANY desktop application, native software, installable client, system utility, productivity tool, IDE plugin, or standalone GUI program for Windows, macOS, or Linux — this instruction set OVERRIDES the general protocol. Read this entirely before generating a single line of code.

---

## SECTION 1: PROJECT INTELLIGENCE & DISCOVERY

### 1.1 Aim Extraction Matrix
Before architecture, document:
- **Primary Objective**: Core problem solved and workflow optimization
- **Target Platforms**: Windows 10/11 / macOS 12+ / Linux (Ubuntu/Debian/Fedora/Arch) / All three
- **Distribution Model**: Microsoft Store / Mac App Store / DMG / MSI / EXE / AppImage / Snap / Flatpak / Homebrew / Chocolatey / Winget / Direct download
- **User Personas**: Power users, casual users, enterprise deployment, developers
- **Windowing Model**: Single window / Multi-window / Tabbed interface / MDI (Multiple Document Interface)
- **System Integration Depth**: System tray, global shortcuts, file associations, protocol handlers, shell extensions, context menus
- **Hardware Access**: GPU acceleration, serial ports, USB devices, Bluetooth, printers, scanners
- **Enterprise Requirements**: MSI with GPO deployment, MDM compatibility, silent install, license server

### 1.2 Input / Output Specification

| Input Channel | Format | Validation | Handling |
|--------------|--------|------------|----------|
| Keyboard | Key codes, modifiers, shortcuts | Key mapping per OS | Global hotkeys, chord shortcuts |
| Mouse | Click, scroll, drag, gesture | Hit testing, hover states | Right-click context menus |
| Touchpad | Multi-touch gestures | OS gesture recognition | Pinch-to-zoom, swipe navigation |
| File system | Any file type via drag-drop | MIME type, magic bytes | Sandbox permissions, scoped access |
| Clipboard | Text, image, HTML, files | Format availability | Read/write with permission |
| Global shortcuts | OS-registered hotkeys | Conflict detection | Register at launch, graceful fallback |
| Command line | Arguments, flags, stdin | Arg parsing (clap, commander) | Headless mode support |
| IPC / Socket | TCP, Unix socket, named pipes | Protocol version check | Single instance enforcement |
| WebSocket | JSON / binary frames | Schema validation | Auto-reconnect, heartbeat |
| USB / Serial | Raw bytes, HID reports | Device enumeration | Permission dialogs, safe removal |
| Microphone | PCM / AAC stream | Permission + level monitoring | Noise gate, VAD (Voice Activity) |
| Camera | Video frames | Resolution negotiation | Background blur, virtual background |

| Output Channel | Format | Optimization | Fallback |
|---------------|--------|--------------|----------|
| Screen rendering | GPU textures / Native widgets | 60fps, VSync, GPU compositing | Software rasterization |
| Audio playback | WAV / MP3 / AAC / Opus | Buffer preloading, gapless | Lower bitrate fallback |
| File export | Any format | Background write, progress dialog | Temp file + atomic rename |
| Print | PDF intermediate | Print preview, page setup | Save to PDF fallback |
| Notifications | Native OS notifications | Rich text, action buttons | In-app notification center |
| System tray | Icon + tooltip + menu | Icon variants (light/dark) | Taskbar fallback on Linux |
| Clipboard | Multi-format write | Delayed rendering | Plain text fallback |
| Protocol handler | Custom URL scheme | URL validation, deep link | Browser fallback |
| External display | HDMI / DisplayPort / AirPlay | Resolution adaptation | Mirror mode |

### 1.3 Working Mechanism Architecture
Document the complete application lifecycle:
1. **Launch Flow**: Splash screen → Config load → Single instance check → Window creation → Tray setup
2. **Process Model**: Main process (backend) + Renderer process(es) (UI) + Utility processes (workers)
3. **Window Lifecycle**: Create → Load → Show → Focus → Minimize → Restore → Hide → Close → Quit
4. **State Persistence**: Window bounds, sidebar widths, column sizes, last opened files, recent documents
5. **Background Operation**: Continue running in system tray after window close (optional)
6. **Auto-Update Flow**: Check → Download → Verify signature → Apply → Restart
7. **Crash Recovery**: Crash reporter, session restore, unsaved change recovery
8. **Power Management**: Sleep/wake awareness, battery saver mode, GPU throttling

---

## SECTION 2: PLATFORM SELECTION & TECH-STACK MANDATE

### 2.1 Cross-Platform Frameworks (Priority Order)

**Tauri 2.0 (Recommended for Performance)**
```
Frontend: React 18+ / Vue 3 / Svelte + TypeScript (strict)
Styling: Tailwind CSS + CSS variables for theming
Backend: Rust (Tokio async runtime)
IPC: Tauri commands (typed, validated) + Events
Storage: SQLite (rusqlite) + Tauri Store plugin
Native APIs: Tauri plugins (notification, dialog, fs, shell, clipboard, globalShortcut, updater, http)
Security: CSP strict, contextIsolation, capability-based permissions
Build: Cargo + Vite
Target: Windows (MSVC), macOS (Clang), Linux (GCC)
```

**Electron 30+ (When Web Ecosystem Critical)**
```
Frontend: React 18+ / Vue 3 + TypeScript
Main Process: Node.js 20 + Electron Forge / electron-builder
Renderer: Chromium engine (embedded)
IPC: ContextBridge with strict channel validation + preload scripts
Storage: electron-store (encrypted) + better-sqlite3
Native APIs: Electron native modules, node-ffi, custom C++ addons
Security: contextIsolation: true, sandbox: true, nodeIntegration: false, enableRemoteModule: false
Build: Electron Forge (webpack/vite), code signing, auto-updater (Squirrel/Sparkle)
```

**Flutter Desktop (When UI Consistency Paramount)**
```
Language: Dart 3.3+ (strict mode)
Framework: Flutter 3.22+ (Windows, macOS, Linux)
State: Riverpod / Bloc / GetX
Storage: Hive + sqflite + SharedPreferences
Native: Platform channels (MethodChannel, EventChannel)
Plugins: desktop_drop, window_manager, system_tray, local_notifier, auto_updater
Build: Flutter build + custom embedder
```

**React Native Desktop (When Mobile Code Reuse)**
```
Framework: React Native Windows + React Native macOS
Language: TypeScript + C++ native modules
Navigation: React Navigation (adapted for desktop)
State: Zustand + TanStack Query
Build: MSBuild (Windows), Xcode (macOS)
```

### 2.2 Native Development (When Maximum Performance Required)

**Windows Native**
```
Framework: WinUI 3 (Windows App SDK) / WPF / MAUI
Language: C# 12 + .NET 8
UI: XAML with MVVM pattern
Storage: Entity Framework Core + SQLite
Distribution: MSIX (Microsoft Store) or MSI (direct)
Interop: P/Invoke for Win32 APIs, COM objects
```

**macOS Native**
```
Framework: SwiftUI (primary) + AppKit (fallback for complex views)
Language: Swift 5.9+
Architecture: MVVM + Combine (reactive) or TCA
Storage: Core Data / SwiftData + CloudKit sync
Distribution: DMG (direct) or Mac App Store (sandboxed)
Interop: Objective-C bridging for legacy frameworks
```

**Linux Native**
```
Framework: GTK4 + libadwaita (GNOME) / Qt6 (KDE/cross-DE)
Language: C / C++ / Vala / Python (PyGObject / PyQt)
Storage: SQLite + GSettings (dconf)
Distribution: Flatpak (universal) / DEB / RPM / AppImage / Snap
```

### 2.3 Shared Backend & Infrastructure
```
Runtime: Node.js 20 / Rust (Tokio) / .NET 8
API: REST / tRPC / gRPC
Auth: OAuth 2.0 + PKCE (desktop flow), JWT refresh tokens
Database: PostgreSQL 16 (cloud sync) + SQLite (local)
File Storage: AWS S3 / Cloudflare R2 with signed URLs
Real-time: WebSocket / SSE
Updates: Tauri updater / Electron autoUpdater / Sparkle (macOS) / WinSparkle (Windows)
Analytics: PostHog / Mixpanel (privacy-respecting, opt-in)
Crash Reporting: Sentry Native / Crashpad / Breakpad
```

---

## SECTION 3: SYSTEM ARCHITECTURE

### 3.1 Desktop Application Layered Architecture
```
┌─────────────────────────────────────────────────────────────┐
│                    PRESENTATION LAYER                        │
│  Main Window → Dialogs → Popovers → Context Menus           │
│  Multi-Window Manager → Tab System → Split Panes            │
│  Custom Title Bar / Frameless Window (optional)             │
├─────────────────────────────────────────────────────────────┤
│                    STATE MANAGEMENT LAYER                    │
│  UI State (Zustand/Riverpod/Redux) → Domain State           │
│  Settings / Preferences → Session → Recent Documents        │
│  Reactive Streams (RxJS / Combine / Dart Streams)           │
├─────────────────────────────────────────────────────────────┤
│                    BUSINESS LOGIC LAYER                      │
│  Use Cases → Services → Managers                            │
│  Background Workers → File Processors → Sync Engine         │
│  Plugin System → Extension API → Scripting Engine           │
├─────────────────────────────────────────────────────────────┤
│                    PLATFORM ABSTRACTION LAYER                │
│  Window Management → System Tray → Native Menus             │
│  File System → Shell Integration → Protocol Handlers        │
│  Notifications → Global Shortcuts → Auto-Updater            │
├─────────────────────────────────────────────────────────────┤
│                    DATA LAYER                                │
│  Local DB (SQLite) → Remote API → Cache (Redis local)       │
│  File System → Key-Value Store → Encrypted Vault            │
│  Search Index (SQLite FTS / Meilisearch local)              │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Process Architecture
**Multi-Process Model (Electron/Tauri)**:
```
Main Process (Node.js / Rust):
├── Window Manager (create, destroy, focus, bounds)
├── System Tray Manager
├── Native Menu Builder
├── Auto-Updater Service
├── IPC Router (commands from renderer)
├── File System Watcher
└── Background Task Scheduler

Renderer Process(es) (Chromium WebView):
├── React/Vue UI Tree
├── State Management
├── Local Storage / IndexedDB
└── IPC Client (invoke Tauri/Electron APIs)

Utility Process (optional):
├── Heavy computation (ML inference, file parsing)
├── Video encoding / image processing
└── Isolated from UI for stability
```

**Single-Process Model (Native/Flutter)**:
```
Main Thread:
├── UI Event Loop
├── Window Management
└── Platform Channel Handler

Background Threads:
├── I/O Operations (file read/write, network)
├── Database Queries
├── Background Jobs
└── Rendering (GPU thread)
```

### 3.3 Window Management Architecture
- **Primary Window**: Main application interface, remembers position/size
- **Secondary Windows**: Detached panels, tool windows, preferences (modal or non-modal)
- **Dialog Windows**: File picker, alerts, confirmations (modal, centered on parent)
- **Splash Window**: Launch screen, loading progress, disappears after init
- **Mini Windows**: Picture-in-picture, floating widgets, always-on-top utilities
- **Window States**: Normal, Minimized, Maximized, Fullscreen, Hidden (tray-only)
- **Window Bounds Persistence**: Save on close, restore on launch, multi-monitor aware

---

## SECTION 4: DESIGN SYSTEM & PLATFORM COMPLIANCE

### 4.1 Platform Design Guidelines (Strict Adherence)

**Windows 11 (Fluent Design)**
```
Visual Language:
- Mica/Acrylic materials (translucent backgrounds with noise texture)
- Rounded corners: 8px standard, 20px for overlays
- Shadows: Layered elevation system
- Accent color: System accent color (respect user preference)
- Light/Dark mode: Auto-detect via registry, manual override
- Title bar: Extend content into title bar (custom caption buttons)
- Snap layouts: Awareness of snap zones (hover maximize button)

Typography:
- Segoe UI (body), Segoe UI Variable (headings), Cascadia Code (monospace)
- Type ramp: Caption → Body → Body Large → Subtitle → Title → Title Large → Display

Spacing:
- 4px base grid
- Control height: 32px standard, 28px compact, 40px expanded
- Button padding: 11px horizontal, 5px vertical
- Corner radius: 4px (buttons), 8px (cards), 20px (dialogs)
```

**macOS (Human Interface Guidelines)**
```
Visual Language:
- Vibrancy effects (NSVisualEffectView: sidebar, title bar)
- Rounded corners: 10px windows, 6px controls
- Shadows: Subtle, directional (light from top)
- Accent color: System preference (blue, purple, pink, red, orange, yellow, green, graphite)
- Dark mode: Auto-detect, respect system schedule
- Title bar: Unified toolbar (content extends into title bar)
- Vibrant sidebar: Source list appearance with selection highlight

Typography:
- SF Pro (body), SF Pro Display (headings), SF Mono (code)
- Dynamic Type support (accessibility sizes)

Spacing:
- 8px base grid
- Toolbar height: 38px standard, 52px expanded
- Button padding: 4px 14px
- Corner radius: 6px (buttons), 10px (text fields), 14px (dialogs)
```

**Linux (GNOME / GTK4)**
```
Visual Language:
- libadwaita styling (rounded, spacious)
- Rounded corners: 12px windows, 6px buttons
- Accent color: Theme-defined (respect user GTK theme)
- Dark mode: Follows GTK theme preference
- Header bar: Content integrated, no separate title bar
- Touch-friendly: Minimum 44px touch targets

Typography:
- Cantarell (GNOME default), Inter (modern), monospace system font

Spacing:
- 6px base grid
- Button height: 34px
- List row height: 44px
- Corner radius: 6px (buttons), 12px (cards), 18px (dialogs)
```

### 4.2 Cross-Platform Design Tokens
```css
/* CSS Variables for Tauri/Electron */
:root {
  /* Elevation / Shadows */
  --shadow-none: none;
  --shadow-sm: 0 1px 2px 0 rgb(0 0 0 / 0.05);
  --shadow: 0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1);
  --shadow-md: 0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1);
  --shadow-lg: 0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1);
  --shadow-xl: 0 20px 25px -5px rgb(0 0 0 / 0.1), 0 8px 10px -6px rgb(0 0 0 / 0.1);
  --shadow-2xl: 0 25px 50px -12px rgb(0 0 0 / 0.25);
  --shadow-inner: inset 0 2px 4px 0 rgb(0 0 0 / 0.05);

  /* Spacing Scale (4px base) */
  --space-0: 0px;
  --space-1: 4px;
  --space-2: 8px;
  --space-3: 12px;
  --space-4: 16px;
  --space-5: 20px;
  --space-6: 24px;
  --space-8: 32px;
  --space-10: 40px;
  --space-12: 48px;
  --space-16: 64px;
  --space-20: 80px;
  --space-24: 96px;

  /* Border Radius */
  --radius-none: 0px;
  --radius-sm: 4px;
  --radius-md: 6px;
  --radius-lg: 8px;
  --radius-xl: 12px;
  --radius-2xl: 16px;
  --radius-3xl: 20px;
  --radius-full: 9999px;

  /* Z-Index Scale */
  --z-base: 0;
  --z-dropdown: 100;
  --z-sticky: 200;
  --z-drawer: 300;
  --z-modal: 400;
  --z-popover: 500;
  --z-tooltip: 600;
  --z-toast: 700;
  --z-overlay: 800;
  --z-max: 9999;
}
```

### 4.3 Typography Scale (Desktop-Optimized)
```
text-xs: 0.75rem / 1rem (12px)     — Captions, badges, timestamps
text-sm: 0.875rem / 1.25rem (14px)  — Secondary text, labels
text-base: 1rem / 1.5rem (16px)    — Body default
text-lg: 1.125rem / 1.75rem (18px) — Lead paragraphs, section labels
text-xl: 1.25rem / 1.75rem (20px)  — H5, card titles
text-2xl: 1.5rem / 2rem (24px)     — H4, dialog titles
text-3xl: 1.875rem / 2.25rem (30px)— H3, major sections
text-4xl: 2.25rem / 2.5rem (36px)  — H2, page headers
text-5xl: 3rem / 1 (48px)          — H1, hero titles
text-6xl: 3.75rem / 1 (60px)       — Display, welcome screens
text-7xl: 4.5rem / 1 (72px)        — Marketing, splash screens

Font Stack:
- Sans-serif: system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif
- Monospace: "JetBrains Mono", "Fira Code", "Cascadia Code", "SF Mono", Consolas, monospace
```

### 4.4 Iconography (Zero Emoji Policy — Strict)
- **Source**: Phosphor Icons (weight: regular, size: 16/20/24/32px) or Lucide (consistent stroke width)
- **Platform variants**: Use native icons where platform convention demands (e.g., macOS uses "Command" symbol, Windows uses "Control")
- **Menu icons**: 16px, aligned with text baseline
- **Toolbar icons**: 20px, with 8px padding
- **Sidebar icons**: 24px, with label below or beside
- **Empty states**: 48-64px, muted color
- **NO EMOJI**: Use `CheckCircle`, `XCircle`, `AlertTriangle`, `Info`, `FolderOpen`, `FileText`, `Settings`, `Search`, `Command`, `Keyboard`, `Monitor`, `Cpu`, `HardDrive`, `Wifi`, `Bluetooth`, `Battery`, `Volume2`, `Maximize2`, `Minimize2`, `X`, `Minus`, `Square` — never Unicode emoji characters
- **Color**: Inherit from parent text color (`currentColor`) or semantic colors

---

## SECTION 5: RESPONSIVE & ADAPTIVE DESIGN (Window Resizing)

### 5.1 Window Size Breakpoints
Desktop apps MUST adapt to arbitrary window sizes, not fixed resolutions:

```
Compact: < 640px width          — Mobile-like sidebar collapse, single pane
Medium: 640px - 1023px width    — Collapsed sidebar (icon only), single pane
Expanded: 1024px - 1439px width — Persistent sidebar, two-pane possible
Large: 1440px - 1919px width    — Full sidebar, multi-pane, detail view
Extra Large: 1920px+ width       — Full layout, multiple sidebars, tool panels

Height considerations:
- Short: < 600px height         — Compact toolbars, reduced padding
- Medium: 600px - 900px         — Standard layout
- Tall: > 900px height          — Expanded lists, more visible content
```

### 5.2 Layout Patterns by Window Size

**Compact Mode (< 640px)**:
- Hamburger menu or bottom navigation (rare on desktop, use for tablet mode)
- Single column content
- Modals become full-screen overlays
- Reduced padding (8px instead of 16px)
- Touch-optimized if tablet mode detected

**Medium Mode (640px - 1023px)**:
- Collapsible sidebar (icons only, expand on hover)
- Single main content area
- Floating action button for primary action (if touch-enabled)
- Tables become horizontally scrollable

**Expanded Mode (1024px - 1439px)**:
- Persistent sidebar (200-240px width)
- Two-column layouts (master-detail)
- Fixed headers, scrollable content
- Contextual side panels (collapsible)

**Large Mode (1440px+)**:
- Multi-pane layouts (sidebar + list + detail)
- Resizable split panes (drag divider)
- Multiple tool windows / floating panels
- Command palette (Cmd+K / Ctrl+K) always available
- Status bar at bottom with system info

### 5.3 Responsive Component Behavior

**Sidebar Navigation**:
- Large: Expanded (200px), text + icon, submenus visible
- Medium: Collapsed (64px), icon only, tooltip on hover
- Compact: Hidden, hamburger toggle, overlay drawer

**Data Tables**:
- Large: All columns visible, resizable, sticky headers
- Medium: Priority columns visible, horizontal scroll
- Compact: Card-based layout (one item per row, stacked info)

**Forms**:
- Large: Multi-column layout (2-3 columns)
- Medium: Two-column layout
- Compact: Single column, full-width inputs

**Modals / Dialogs**:
- Large: Centered, max-width 600px, backdrop blur
- Medium: Centered, max-width 90%
- Compact: Full-screen or bottom sheet

### 5.4 Multi-Monitor Awareness
- **Window restore**: Remember which monitor the window was on
- **Monitor change detection**: Adjust scaling if moved to different DPI monitor
- **DPI scaling**: Support 100%, 125%, 150%, 175%, 200%, 250%
- **Per-monitor DPI**: Windows 10+ per-monitor V2 awareness
- **Fullscreen on specific monitor**: Remember monitor for fullscreen mode

---

## SECTION 6: NATIVE INTEGRATION & SYSTEM FEATURES

### 6.1 System Tray / Status Bar Integration
```
Required Features:
- Tray icon: 16x16px (Windows), 18x18px template (macOS), 22x22px (Linux)
- Icon variants: Light mode, dark mode, active state, notification badge
- Context menu: Show/Hide window, Recent items, Settings, Quit
- Left-click behavior: Toggle window visibility (Windows/Linux), Show menu (macOS)
- Tooltip: App name + status (e.g., "App Name - 3 tasks running")
- Notification badge: Red dot or number overlay on tray icon

Implementation:
- Windows: NOTIFYICONDATA (Win32 API)
- macOS: NSStatusItem (system tray) or NSDockTile (dock badge)
- Linux: AppIndicator / StatusNotifierItem / XEmbed fallback
```

### 6.2 Global Shortcuts (System-Wide Hotkeys)
```
Registration:
- Check conflicts with OS shortcuts (warn user if conflict)
- Allow user customization in settings
- Store in key-value storage
- Register at app launch, unregister at quit

Common Shortcuts:
- Show/Hide app: Ctrl+Shift+A (customizable)
- Quick action: Ctrl+Shift+Space (command palette)
- Screenshot: Ctrl+Shift+S
- New item: Ctrl+N
- Search: Ctrl+F (in-app), Ctrl+Shift+F (global search)

Platform differences:
- macOS: Use Cmd instead of Ctrl, Option instead of Alt
- Windows/Linux: Ctrl + Alt + Shift modifiers
- Respect accessibility: Allow single-key shortcuts to be disabled
```

### 6.3 Native Application Menu
**macOS Menu Bar** (Mandatory):
```
App Name
├── About App Name
├── Check for Updates...
├── Preferences... (Cmd+,)
├── Services (submenu)
├── Hide App Name (Cmd+H)
├── Hide Others (Cmd+Option+H)
├── Show All
└── Quit App Name (Cmd+Q)

File
├── New (Cmd+N)
├── Open... (Cmd+O)
├── Open Recent (submenu with clear)
├── Close (Cmd+W)
├── Save (Cmd+S)
├── Save As... (Cmd+Shift+S)
└── Export...

Edit
├── Undo (Cmd+Z)
├── Redo (Cmd+Shift+Z)
├── Cut (Cmd+X)
├── Copy (Cmd+C)
├── Paste (Cmd+V)
├── Select All (Cmd+A)
└── Find (Cmd+F)

View
├── Reload (Cmd+R)
├── Toggle Full Screen (Ctrl+Cmd+F)
├── Toggle Developer Tools (Cmd+Option+I)
└── Actual Size / Zoom In / Zoom Out

Window
├── Minimize (Cmd+M)
├── Zoom
├── Tile Window to Left/Right
├── Bring All to Front
└── [List of open windows]

Help
├── Documentation
├── Keyboard Shortcuts
├── Report Issue
└── About
```

**Windows/Linux Menu Bar** (Optional, prefer hamburger or ribbon):
- File / Edit / View / Tools / Help structure
- Accelerators: Alt+F for File, underlined letters
- Context-sensitive enable/disable

### 6.4 File System Integration
- **File associations**: Register as default app for specific extensions (`.myapp`, `.pdf`, etc.)
- **Open with**: Appear in "Open with" context menu
- **Recent documents**: Track in OS recent files + app-specific recent list
- **Drag and drop**: Accept files from Explorer/Finder onto app window
- **File watching**: Watch working directory for external changes, prompt reload
- **Auto-save**: Save drafts to temp location, recover on crash
- **Export**: Multiple formats with progress dialog

### 6.5 Protocol Handlers & Deep Linking
- **Custom URL scheme**: `myapp://action/param` (e.g., `myapp://open/file/path`)
- **Web URL association**: Universal Links (iOS) / App Links (Android) equivalent for desktop
- **OAuth callback**: Handle `myapp://auth/callback?token=xyz` for login flows
- **Single instance enforcement**: If app already running, forward URL to existing instance
- **Command line arguments**: Support `myapp.exe --open file.txt --theme dark`

### 6.6 Notifications
- **Local notifications**: Scheduled reminders, task due, export complete
- **Rich notifications**: Action buttons (Reply, Dismiss, Snooze), progress bars
- **Notification grouping**: By category (messages, tasks, system)
- **Do Not Disturb awareness**: Respect OS focus mode / DND status
- **Notification history**: In-app notification center for missed alerts
- **Badge count**: Dock (macOS) / taskbar (Windows) / launcher (Linux)

### 6.7 Auto-Updater
```
Update Flow:
1. Check for updates on launch (background, non-blocking)
2. If update available: Show subtle indicator (badge, menu item)
3. Download in background (delta update preferred)
4. Verify signature/certificate before applying
5. Prompt user: "Restart to apply update" or "Update on quit"
6. Apply update atomically (replace on restart)
7. Rollback capability if new version fails to launch

Channels:
- Stable: Production releases
- Beta: Pre-release for testing
- Nightly: Latest builds (developer only)

User control:
- Check frequency: On launch, daily, weekly, manual only
- Auto-download: Yes/No
- Auto-install: Yes/No (prompt first)
```

### 6.8 Window Controls & Behavior
- **Custom title bar**: Optional, extend content into title bar (frameless)
- **Window controls**: Close (X), Minimize (-), Maximize/Restore (square), Fullscreen
- **Window snapping**: Snap to edges, corners, half-screen (Windows 11 style)
- **Always on top**: Pin window above others (utility apps)
- **Minimize to tray**: Close button minimizes to system tray instead of quitting
- **Multi-window**: Support multiple independent windows with shared state
- **Tabbed interface**: Chrome-style tabs within single window (optional)
- **Picture-in-picture**: Small floating window, always on top

---

## SECTION 7: COMPONENT ARCHITECTURE

### 7.1 Desktop-Specific Component Inventory

**Window Chrome**:
- Custom title bar (draggable region, window controls, menu buttons)
- Traffic lights (macOS style: close/minimize/zoom circles)
- Caption buttons (Windows style: minimize/maximize/close icons)
- Window drag region (CSS `-webkit-app-region: drag`)
- Window resize handles (8 directions)

**Navigation**:
- Sidebar (collapsible, resizable, icon + text, nested sections)
- Top navigation bar (breadcrumbs, tabs, window controls)
- Command palette (Cmd+K, fuzzy search, actions, recent files)
- Toolbar (icon buttons, dropdowns, segmented controls)
- Status bar (bottom: info, progress, zoom, sync status, update indicator)

**Layout**:
- Resizable split panes (drag divider, collapse/expand, min/max sizes)
- Docking panels (drag to dock left/right/bottom/float)
- Tab system (reorderable, closable, context menu, drag between windows)
- Master-detail view (list on left, detail on right, responsive)
- Grid layout (draggable cards, responsive columns)
- Floating panels (tool windows, inspectors, palettes)

**Data Display**:
- Data table (sortable, filterable, resizable columns, row selection, virtualized)
- Tree view (expandable, multi-select, drag-drop reorder, search)
- File browser (icon/list/details view, breadcrumb, preview pane)
- Code editor (Monaco Editor / CodeMirror integration, syntax highlighting)
- Terminal / Console (embedded, color themes, command history)
- Kanban board (drag-drop columns, cards, swimlanes)
- Calendar (month/week/day views, events, drag to reschedule)
- Chart dashboard (line, bar, pie, real-time updates)

**Input**:
- Text input with autocomplete, history, validation
- Rich text editor (TipTap / Slate / Quill, markdown support)
- Code input (syntax highlighting, linting, IntelliSense)
- File picker (native dialog, drag-drop zone, recent files)
- Color picker (native OS picker + custom palette)
- Date/time picker (native OS picker preferred)
- Keyboard shortcut recorder (capture key combination, validate conflicts)
- Slider with value display, dual-handle range slider

**Feedback**:
- Toast notifications (top-right corner, auto-dismiss, action buttons)
- Inline validation (red border + message, shake animation)
- Progress indicators (linear with percentage, circular, indeterminate, multi-step)
- Loading skeletons (match exact layout structure)
- Empty states (illustration + headline + description + CTA)
- Error boundaries (crash screen with reload button, report issue)
- Confirmation dialogs (destructive actions highlighted in red)
- Unsaved changes indicator (dot in title bar, prompt on close)

### 7.2 Component Props Interface Standard
```typescript
interface DesktopComponentProps {
  // Visual
  variant?: "default" | "primary" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md" | "lg";
  
  // State
  isLoading?: boolean;
  isDisabled?: boolean;
  isActive?: boolean;
  
  // Desktop-specific
  shortcut?: string;           // Keyboard shortcut display (e.g., "Cmd+S")
  tooltip?: string;            // Hover tooltip text
  contextMenu?: MenuItem[];    // Right-click context menu items
  
  // Accessibility
  ariaLabel?: string;
  ariaDescribedBy?: string;
  
  // Events
  onClick?: (event: MouseEvent) => void;
  onDoubleClick?: (event: MouseEvent) => void;
  onContextMenu?: (event: MouseEvent) => void;
  onKeyDown?: (event: KeyboardEvent) => void;
}
```

---

## SECTION 8: STATE MANAGEMENT & DATA ARCHITECTURE

### 8.1 Desktop State Categories
```
UI State (Ephemeral):
- Window bounds (x, y, width, height, maximized, fullscreen)
- Sidebar width, panel visibility, split pane sizes
- Active tab, selected items, scroll positions
- Modal open/close, tooltip visibility
- Command palette query, filter states

App State (Persistent across sessions):
- User preferences (theme, language, font size, shortcuts)
- Recent documents (last 20, with pinned items)
- Window layout (sidebar collapsed, panel arrangement)
- Account/session state
- Feature flags, onboarding completion

Document / Project State:
- Open files/projects with unsaved changes
- Undo/redo stacks per document
- Selection state, cursor position, zoom level
- Search/replace history

Server State (Sync with cloud):
- User profile, settings sync
- Shared documents, collaboration cursors
- Notifications, messages
- License/subscription status
```

### 8.2 State Persistence Strategy
```typescript
// Settings store with schema validation
interface AppSettings {
  window: {
    bounds: { x: number; y: number; width: number; height: number };
    maximized: boolean;
    sidebarWidth: number;
    sidebarCollapsed: boolean;
  };
  appearance: {
    theme: "system" | "light" | "dark";
    accentColor: string;
    fontSize: "small" | "medium" | "large";
    density: "compact" | "comfortable" | "spacious";
  };
  behavior: {
    minimizeToTray: boolean;
    closeToTray: boolean;
    autoSave: boolean;
    autoSaveInterval: number;
    checkUpdates: "startup" | "daily" | "weekly" | "manual";
    defaultFolder: string;
  };
  shortcuts: Record<string, string>;
  recentDocuments: Array<{ path: string; name: string; timestamp: number; pinned: boolean }>;
}

// Persistence layers:
// 1. Tauri: tauri-plugin-store (JSON file in app config dir)
// 2. Electron: electron-store (encrypted, schema-validated)
// 3. Native: UserDefaults (macOS), Registry (Windows), dconf (Linux)
```

### 8.3 Undo / Redo Architecture
```
Command Pattern Implementation:
- Every user action = Command object
- Command.execute() → modifies state
- Command.undo() → reverses modification
- Command.redo() → re-applies modification
- CommandStack: Array<Command> with current index pointer
- Max stack size: 100 commands (configurable)
- Grouped commands: Begin batch → multiple commands → End batch (single undo)
- Persistent undo: Save undo stack with document (optional)
```

---

## SECTION 9: PERFORMANCE OPTIMIZATION

### 9.1 Startup Performance
- **Cold start target**: < 1.5 seconds to interactive window
- **Splash screen**: Native splash (not JS-rendered), immediate on process start
- **Lazy loading**: Defer non-critical modules until after window shown
- **Preload strategy**: Preload next likely screen while user reads current
- **Native module initialization**: Parallelize, non-blocking
- **Database migration**: Run on background thread, show progress if slow

### 9.2 Runtime Performance
- **Frame rate**: 60fps for all animations, 120fps on high-refresh displays
- **Memory budget**: < 200MB RAM for typical usage, < 500MB for heavy workloads
- **GPU acceleration**: Use WebGL/Metal/DirectX for complex rendering, not CPU
- **Virtualization**: Lists with 1000+ items must virtualize (react-window, Flutter ListView.builder)
- **Image handling**: Lazy load, proper sizing, WebP/AVIF format, memory cache with LRU eviction
- **Background tasks**: Web Workers / Rust threads for file processing, compression, encoding

### 9.3 Bundle Size Optimization
- **Tree shaking**: ES modules, sideEffects: false
- **Code splitting**: Route-based, lazy load settings, about, help pages
- **Native dependencies**: Audit and remove unused native modules
- **Asset optimization**: Compress images, SVG optimization, font subsetting
- **Optional features**: Plugin architecture, load features on demand

### 9.4 Power & Resource Efficiency
- **Idle detection**: Reduce polling, pause animations when window hidden
- **Background throttling**: Reduce JS timer resolution when minimized
- **Network batching**: Queue API calls, flush on interval or threshold
- **File watcher**: Use OS-native watchers (FSEvents, ReadDirectoryChanges, inotify) not polling
- **Memory pressure**: Release caches, dispose unused resources on OS memory warning

---

## SECTION 10: SECURITY HARDENING

### 10.1 Process Security
- **Context isolation** (Electron): Renderer cannot access Node.js APIs directly
- **Sandbox** (Electron): Renderer runs in OS sandbox
- **CSP**: Strict Content Security Policy preventing inline scripts
- **IPC validation**: All IPC messages validated against Zod schemas
- **No eval()**: Disable `eval()`, `new Function()`, `setTimeout(string)`
- **No remote**: `enableRemoteModule: false` (Electron legacy)

### 10.2 Data Protection
- **Encryption at rest**: SQLCipher for SQLite, OS keychain for secrets
- **Secure storage**: Keychain (macOS), Credential Manager (Windows), Secret Service (Linux)
- **Memory safety**: Zero sensitive data from memory after use
- **File permissions**: User documents directory only, no system file access without explicit permission

### 10.3 Update Security
- **Signature verification**: Verify code signature before applying update
- **HTTPS only**: All update checks and downloads over TLS 1.3
- **Checksum validation**: SHA-256 hash verification of downloaded update
- **Rollback**: Keep previous version for 7 days, auto-rollback if new version crashes on launch

### 10.4 Network Security
- **Certificate pinning**: Pin server certificate for high-security apps
- **No mixed content**: Block HTTP resources when loaded over HTTPS
- **CORS**: Strict origin whitelist for API calls
- **Request signing**: HMAC-SHA256 signed requests for sensitive APIs

---

## SECTION 11: TESTING STRATEGY

### 11.1 Test Categories
**Unit Tests**:
- Business logic, utilities, formatters
- State management reducers
- IPC command handlers
- Validation logic

**Component Tests**:
- Render tests (does window render without crash?)
- Interaction tests (click, keyboard, drag-drop)
- Accessibility tests (labels, roles, focus management)

**Integration Tests**:
- Native API integration (file system, notifications, shortcuts)
- Database operations
- IPC communication between main and renderer
- Auto-updater flow (mocked)

**E2E Tests**:
- Full user journey: Install → Launch → Setup → Use Feature → Quit
- Window management: Resize, maximize, minimize, restore, multi-monitor
- File operations: Open, edit, save, export
- Critical paths only (too slow for full coverage)

### 11.2 Desktop-Specific Testing
- **Window state tests**: Restore bounds, maximize, fullscreen, minimize to tray
- **Multi-monitor tests**: Move window between monitors, DPI change, disconnect monitor
- **DPI scaling tests**: 100%, 125%, 150%, 200%, 250%
- **Platform tests**: Run on Windows 10/11, macOS 12+, Ubuntu 22.04+
- **Installer tests**: Silent install, upgrade, uninstall (no leftovers)
- **Auto-update tests**: Check, download, apply, rollback scenarios

### 11.3 Test Matrix
Minimum test coverage:
- **Windows**: Windows 10 (21H2), Windows 11 (23H2), different DPIs
- **macOS**: macOS 12 (Monterey), macOS 13 (Ventura), macOS 14 (Sonoma), Intel + Apple Silicon
- **Linux**: Ubuntu 22.04 LTS, Fedora 39, Arch (latest), X11 + Wayland

---

## SECTION 12: BUILD, SIGNING & DISTRIBUTION

### 12.1 Build Configuration

**Windows**:
- Target: x64, ARM64 (optional)
- Build: MSI (enterprise), EXE (direct), MSIX (Microsoft Store)
- Signing: EV Code Signing Certificate (SmartScreen bypass) or Standard Code Signing
- Installer: WiX Toolset (MSI), NSIS (EXE), MSIX Packaging Tool
- Dependencies: Visual C++ Redistributable bundled or checked
- Registry: Add uninstall entry, file associations, protocol handlers

**macOS**:
- Target: x64, ARM64 (universal binary preferred)
- Build: DMG (direct), PKG (enterprise), App Bundle (Mac App Store)
- Signing: Apple Developer ID Application certificate (direct) or Mac Distribution (App Store)
- Notarization: Required for macOS 10.15+, staple ticket to app
- Hardened Runtime: Enabled with entitlements (JIT if needed for JS engine)
- Sandboxing: App Store requires, direct distribution optional

**Linux**:
- Target: x64, ARM64
- Formats: AppImage (universal), DEB (Debian/Ubuntu), RPM (Fedora), Snap (Ubuntu Store), Flatpak (Flathub)
- Dependencies: Bundle or declare in package metadata
- Desktop entry: `.desktop` file with icon, categories, MIME types
- AppArmor/SELinux: Profile for security (optional but recommended)

### 12.2 CI/CD Pipeline
```yaml
jobs:
  lint-and-typecheck:
    runs-on: ubuntu-latest
    steps:
      - checkout
      - install dependencies
      - run lint (ESLint / Clippy / Dart analyze)
      - run type check
      - run unit tests

  build-windows:
    runs-on: windows-latest
    needs: lint-and-typecheck
    steps:
      - checkout
      - install dependencies
      - build release binary
      - sign with AzureSignTool or signtool
      - build installer (MSI/EXE)
      - sign installer
      - upload artifact

  build-macos:
    runs-on: macos-latest
    needs: lint-and-typecheck
    steps:
      - checkout
      - install dependencies
      - build release binary (universal)
      - sign with codesign
      - notarize with notarytool
      - staple notarization ticket
      - build DMG
      - upload artifact

  build-linux:
    runs-on: ubuntu-latest
    needs: lint-and-typecheck
    steps:
      - checkout
      - install dependencies
      - build release binary
      - build AppImage
      - build DEB package
      - build Flatpak (optional)
      - upload artifacts

  e2e-tests:
    needs: [build-windows, build-macos, build-linux]
    strategy:
      matrix:
        os: [windows-latest, macos-latest, ubuntu-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - run E2E tests on built binary
      - upload screenshots on failure

  release:
    if: startsWith(github.ref, 'refs/tags/v')
    needs: e2e-tests
    steps:
      - create GitHub Release
      - upload all platform artifacts
      - update auto-updater manifest (latest.yml, latest-mac.yml, latest-linux.yml)
```

---

## SECTION 13: DEPLOYMENT & OPERATIONS

### 13.1 Distribution Channels
- **Direct Download**: Website with platform auto-detection, delta updates
- **Microsoft Store**: MSIX packaging, automatic updates, broader reach
- **Mac App Store**: Sandboxed, automatic updates, discoverability
- **Homebrew**: `brew install --cask myapp` for macOS
- **Chocolatey / Winget**: `choco install myapp` / `winget install MyApp` for Windows
- **Snap Store**: `snap install myapp` for Ubuntu
- **Flathub**: `flatpak install flathub com.myapp.MyApp` for universal Linux
- **Enterprise**: MSI with Group Policy, silent install flags, license server

### 13.2 Update Infrastructure
- **Update server**: Static JSON manifest + binary files on S3/R2/CloudFront
- **Manifest format**: Version, release notes, download URL, signature, minimum version
- **Delta updates**: Binary diff (bsdiff or similar) to reduce download size
- **Rollback**: Keep N previous versions, allow downgrade if critical bug
- **Force update**: Block usage if security patch required (configurable grace period)

### 13.3 Monitoring & Observability
- **Crash reporting**: Sentry Native, Crashpad, or custom minidump uploader
- **Analytics**: Opt-in only, privacy-respecting, no PII, aggregate metrics
- **Performance**: Startup time, frame drops, memory usage, file operation latency
- **Error logs**: Structured JSON logs, 30-day retention, user-accessible log viewer
- **Update metrics**: Check rate, download rate, install rate, failure rate

---

## SECTION 14: DOCUMENTATION REQUIREMENTS

### 14.1 README.md Structure
```markdown
# App Name

[![Windows](https://img.shields.io/badge/Windows-Download-blue)](link)
[![macOS](https://img.shields.io/badge/macOS-Download-lightgrey)](link)
[![Linux](https://img.shields.io/badge/Linux-Download-orange)](link)
[![CI](https://github.com/user/repo/actions/workflows/ci.yml/badge.svg)](link)

> One-line description of the desktop application.

[Website](link) · [Documentation](link) · [Changelog](link) · [Report Bug](link)

## Features

- [x] Feature one with icon (CheckCircle)
- [x] Feature two
- [x] Feature three

## Screenshots

| Windows | macOS | Linux |
|---------|-------|-------|
| img     | img   | img   |

## Tech Stack

- **Frontend**: React 18 + TypeScript + Tailwind CSS
- **Backend**: Rust (Tauri) / Node.js (Electron)
- **Storage**: SQLite + OS keychain
- **Build**: Vite + Cargo / Electron Forge

## Quick Start

```bash
# 1. Clone
git clone https://github.com/user/repo.git
cd repo

# 2. Install
npm install

# 3. Run (development)
npm run tauri dev  # or npm run electron:dev

# 4. Build (production)
npm run tauri build  # or npm run electron:build
```

## Keyboard Shortcuts

| Action | Windows/Linux | macOS |
|--------|---------------|-------|
| New File | Ctrl+N | Cmd+N |
| Open | Ctrl+O | Cmd+O |
| Save | Ctrl+S | Cmd+S |
| Command Palette | Ctrl+Shift+P | Cmd+Shift+P |
| Toggle Sidebar | Ctrl+B | Cmd+B |
| Fullscreen | F11 | Ctrl+Cmd+F |

## Architecture

[Insert architecture diagram]

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md)

## License

MIT (c) [Your Name]
```

### 14.2 Additional Documentation
- `ARCHITECTURE.md` — Process model, IPC design, state flow
- `NATIVE_APIS.md` — Custom native modules, platform-specific code
- `SHORTCUTS.md` — Complete keyboard shortcut reference
- `BUILD.md` — Platform-specific build instructions, signing setup
- `DEPLOYMENT.md` — Release process, store submission, update server
- `docs/PLUGIN_API.md` — Extension/plugin development guide
- `PRIVACY.md` — Data collection, local storage, network usage

---

## SECTION 15: PROJECT-SPECIFIC ADAPTATIONS

### 15.1 IDE / Code Editor
Additional requirements:
- Monaco Editor or CodeMirror integration
- Syntax highlighting for 50+ languages
- LSP (Language Server Protocol) client integration
- File tree explorer with git status indicators
- Integrated terminal (xterm.js / node-pty)
- Multi-cursor editing, find/replace across files
- Plugin system for extensions
- Git integration (diff viewer, blame, log)
- Debug adapter protocol support

### 15.2 Productivity / Note-Taking
Additional requirements:
- Rich text editor with markdown support
- File organization (folders, tags, links)
- Full-text search (SQLite FTS or Meilisearch local)
- Sync engine with conflict resolution
- Export to PDF, HTML, Markdown
- Templates and snippets
- Daily notes / journaling mode
- Backlinks and graph view

### 15.3 Media / Creative Tools
Additional requirements:
- GPU-accelerated canvas (WebGL / Metal / DirectX)
- Timeline / sequencer interface
- Layer management panel
- Real-time preview
- Export presets (multiple formats, resolutions)
- Plugin architecture for effects/filters
- Hardware acceleration for encoding
- Color management (ICC profiles)

### 15.4 System Utilities
Additional requirements:
- System tray-only mode (no window on launch)
- Global hotkeys for quick actions
- System information display (CPU, memory, network)
- File system watcher with actions
- Clipboard history manager
- Window management tools (tiling, snapping)
- Launch on startup (OS integration)
- Minimal resource footprint (< 50MB RAM)

### 15.5 Communication / Collaboration
Additional requirements:
- Real-time messaging (WebSocket)
- Voice/video calls (WebRTC)
- Screen sharing (getDisplayMedia)
- File sharing (P2P or relay)
- Presence indicators
- Notification management
- Multi-account support
- End-to-end encryption (Signal Protocol)

---

## SECTION 16: FINAL DELIVERY CHECKLIST

### Code Quality
- [ ] TypeScript strict mode / Rust clippy zero warnings / Dart analyze clean
- [ ] No console.log / print in production code
- [ ] Minimum 10,000 lines of production code
- [ ] No emoji characters anywhere — verified with grep
- [ ] All TODOs resolved or ticketed

### Platform Compliance
- [ ] Windows: Runs on Windows 10 21H2+ and Windows 11, respects accent color, light/dark mode
- [ ] macOS: Runs on macOS 12+, native menu bar, proper bundle structure, notarized
- [ ] Linux: Runs on Ubuntu 22.04+, Fedora 39+, respects GTK theme, `.desktop` entry

### Functionality
- [ ] All features from requirements implemented
- [ ] Responsive window resizing works at all breakpoints
- [ ] Multi-monitor support tested
- [ ] DPI scaling correct at 100%, 125%, 150%, 200%
- [ ] System tray integration functional
- [ ] Global shortcuts work (and are customizable)
- [ ] Native menus complete (macOS menu bar, Windows/Linux hamburger/ribbon)
- [ ] File drag-and-drop works from OS file manager
- [ ] File associations registered and functional
- [ ] Protocol handlers respond to custom URLs
- [ ] Auto-updater checks, downloads, and applies updates
- [ ] Window state persists and restores correctly
- [ ] Background operation (minimize to tray) works
- [ ] Command palette (Cmd+K) functional with all actions

### Performance
- [ ] Cold start < 1.5 seconds
- [ ] 60fps maintained during all animations
- [ ] Memory usage < 200MB typical
- [ ] No memory leaks after 8 hours runtime
- [ ] Idle CPU usage < 1%

### Security
- [ ] Context isolation enabled (Electron) / safe IPC (Tauri)
- [ ] CSP headers configured and strict
- [ ] No hardcoded secrets
- [ ] Auto-update signature verification implemented
- [ ] Local data encrypted (SQLCipher or OS keychain)

### Testing
- [ ] Unit test coverage > 70%
- [ ] Integration tests for native APIs
- [ ] E2E tests for critical flows on all platforms
- [ ] Installer tested (install, upgrade, uninstall)
- [ ] Auto-update tested end-to-end

### DevOps
- [ ] CI/CD pipeline builds for all three platforms
- [ ] Code signing configured for Windows and macOS
- [ ] macOS notarization working
- [ ] Linux packages built (AppImage + DEB minimum)
- [ ] Update manifest auto-generated on release

### Documentation
- [ ] README with screenshots per platform
- [ ] Complete keyboard shortcut reference
- [ ] Architecture diagram
- [ ] Build instructions for all platforms
- [ ] Changelog updated
- [ ] Privacy policy documented

---

## MANDATORY OUTPUT SEQUENCE

When generating a desktop application for a user, produce deliverables in this order:

1. **Analysis** — Platform selection, windowing model, system integration depth, distribution strategy
2. **Architecture** — Process model, IPC design, state management, native API integration plan
3. **Design System** — Platform-specific tokens (Fluent / HIG / GTK), responsive breakpoints, iconography
4. **Project Structure** — Complete folder tree with module purposes, native code organization
5. **Core Implementation** — Config, window management, system tray, native menus, IPC layer, settings
6. **Feature Implementation** — Screen-by-screen, feature-by-feature code
7. **Native Integration** — Global shortcuts, file system, notifications, auto-updater, protocol handlers
8. **Backend Integration** — API client, sync engine, real-time features, offline support
9. **Testing Suite** — Unit, integration, E2E, platform-specific tests
10. **Build & Deploy** — CI/CD, code signing, installer config, store metadata, update server
11. **Documentation** — README, shortcut reference, architecture docs, build guide

**REMEMBER**:
- NO EMOJIS — use Phosphor Icons, Lucide, SF Symbols, or Material Icons
- MINIMUM 10,000 lines of production-grade code
- EVERY instruction above must be visibly satisfied
- Desktop apps must feel native to their platform — no web-app-in-a-box feeling
- Respect OS conventions: menus, shortcuts, window behavior, system integration
- When in doubt, over-engineer rather than under-deliver

---

*End of GOVINDA CLI Desktop Application Protocol v4.0*