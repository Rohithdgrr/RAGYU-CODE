 # GOVINDA CLI — MOBILE & DESKTOP APP DEVELOPMENT PROTOCOL
## Version 3.0 | Native, Cross-Platform & Hybrid Application Blueprint

---

## PROTOCOL ACTIVATION
When user prompt implies ANY mobile application (iOS, Android, cross-platform), desktop software (Windows, macOS, Linux), or installable client application — this instruction set OVERRIDES the general protocol. Read this entirely before generating a single line of code.

---

## SECTION 1: PROJECT INTELLIGENCE & DISCOVERY

### 1.1 Aim Extraction Matrix
Before architecture, document:
- **Primary Objective**: Core problem solved and value proposition
- **Target Platforms**: iOS / Android / Windows / macOS / Linux / Web (PWA fallback)
- **Distribution Model**: App Store / Play Store / Microsoft Store / Mac App Store / Direct DMG/EXE/MSI / Enterprise MDM
- **User Personas**: Primary (daily active), secondary (occasional), power users
- **Offline Requirement**: Full offline capability, partial cache, or online-only
- **Hardware Integration**: Camera, GPS, Bluetooth, NFC, Biometrics, Accelerometer, Microphone
- **Regulatory Scope**: GDPR, CCPA, HIPAA, SOC-2, App Store privacy requirements

### 1.2 Input / Output Specification

| Input Channel | Format | Validation | Handling |
|--------------|--------|------------|----------|
| Touch gestures | Multi-touch events | Gesture recognizer bounds | Haptic feedback on action |
| Hardware buttons | Key events | OS interception rules | Override only when appropriate |
| Camera input | RAW / JPEG / HEIC / Video | Resolution + size limits | Background compression queue |
| GPS location | Lat/Lng + accuracy radius | Permission state check | Background location modes |
| Bluetooth/BLE | Byte streams / GATT | Pairing state, signal strength | Connection retry logic |
| Push notifications | JSON payload (4KB limit) | APNs/FCM format validation | Deep link routing |
| File system | Documents / Media / Cache | Sandbox permissions, quota | Scoped storage (Android 10+) |
| Biometric | Boolean / Token | Hardware availability check | Fallback to PIN/Password |
| Microphone | PCM / AAC audio | Permission + recording state | Noise cancellation, encoding |
| Network state | WiFi / Cellular / Offline | Reachability monitoring | Queue actions for sync |

| Output Channel | Format | Optimization | Fallback |
|---------------|--------|--------------|----------|
| Screen rendering | GPU textures / Native views | 60fps target, 120fps support | Software renderer fallback |
| Audio playback | AAC / MP3 / Opus | Buffer preloading | Lower bitrate fallback |
| Haptic feedback | Vibration patterns | System intensity curves | Visual feedback if disabled |
| Notifications | Local + Remote | Rich media attachments | In-app notification center |
| File export | PDF / CSV / Image | Background generation | Share sheet integration |
| Print | AirPrint / Android Print | PDF intermediate | Save to files fallback |
| External display | HDMI / AirPlay / Cast | Resolution adaptation | Mirror mode fallback |

### 1.3 Working Mechanism Architecture
Document the complete application lifecycle:
1. **App States**: Not Running → Background → Inactive → Active → Background → Suspended
2. **Memory Pressure Handling**: Save state, release caches, alert user if critical
3. **Battery Optimization**: Background task scheduling, network batching, location accuracy trade-offs
4. **Concurrency Model**: Main thread (UI only) → Background threads (I/O, compute) → Dedicated queues (audio, video, BLE)
5. **State Persistence**: User defaults / SharedPreferences → SQLite / Realm → Cloud sync
6. **Update Mechanism**: In-app updates (Android), App Store prompt (iOS), Auto-updater (desktop)
7. **Crash Recovery**: Last known state restoration, graceful degradation, error reporting

---

## SECTION 2: PLATFORM SELECTION & TECH-STACK MANDATE

### 2.1 Mobile Cross-Platform (Recommended Default)
```
Framework: React Native 0.74+ (New Architecture / Bridgeless) OR Flutter 3.22+
Language: TypeScript (RN) / Dart (Flutter) — strict mode enforced
State Management: Zustand + TanStack Query (RN) / Riverpod + Dio (Flutter)
Navigation: React Navigation v6+ (RN) / GoRouter (Flutter)
Storage: MMKV (RN) / Hive + SQLite (Flutter)
Networking: Axios + React Query (RN) / Dio + Retrofit pattern (Flutter)
Forms: React Hook Form + Zod (RN) / Flutter Formz + Freezed (Flutter)
Animation: Reanimated 3 (RN) / Flutter implicit + explicit animations
Icons: Phosphor Icons / Lucide (RN) / Material + Cupertino (Flutter)
Maps: React Native Maps / Google Maps Flutter
Push: Notifee + Firebase (RN) / Firebase Messaging + Local Notifications (Flutter)
Biometrics: React Native Biometrics / Local Auth (Flutter)
Camera: React Native VisionCamera / Camera Awesome (Flutter)
```

### 2.2 Native iOS (When Required)
```
Language: Swift 5.9+ (SwiftUI for UI, UIKit for complex custom views)
Architecture: MVVM-C (Model-View-ViewModel-Coordinator) or TCA (The Composable Architecture)
Concurrency: Swift Concurrency (async/await), Actors for thread safety
Persistence: Core Data / SwiftData (iOS 17+) / GRDB (SQLite wrapper)
Networking: URLSession + Codable + Alamofire (complex cases)
Dependency Injection: Swinject or manual constructor injection
Testing: XCTest + XCTestUI + SnapshotTesting
Build Tool: Xcode 15+, Swift Package Manager
Minimum iOS Version: iOS 15+ (covering 95%+ devices)
```

### 2.3 Native Android (When Required)
```
Language: Kotlin 1.9+ (Coroutines + Flow)
Architecture: MVVM + Clean Architecture + MVI for complex screens
UI: Jetpack Compose (primary) + XML legacy support only if required
Concurrency: Kotlin Coroutines (Dispatchers.IO, .Main, .Default)
Persistence: Room (SQLite abstraction) / DataStore (preferences) / Realm
Networking: Ktor Client / Retrofit + OkHttp + Kotlin Serialization
Dependency Injection: Hilt (Dagger wrapper)
Testing: JUnit 5 + Espresso + UI Automator + MockK
Build Tool: Gradle Kotlin DSL
Minimum SDK: API 26 (Android 8.0) — covering 92%+ devices
Target SDK: API 34 (Android 14)
```

### 2.4 Desktop Applications
```
Cross-Platform: Tauri 2.0 (Rust + WebView) OR Electron 30+ (Node + Chromium)
Native Windows: C# + WPF / WinUI 3 / MAUI
Native macOS: Swift + SwiftUI / AppKit
Native Linux: GTK4 + Vala / Qt6 + C++

Tauri Stack (Recommended for performance):
- Frontend: React 18 + TypeScript + Tailwind (embedded in WebView)
- Backend: Rust (Tokio async runtime)
- Storage: SQLite (rusqlite) + LocalStorage (frontend cache)
- Notifications: tauri-plugin-notification
- Auto-updater: tauri-plugin-updater
- Native APIs: Rust commands exposed to frontend via IPC

Electron Stack (When web ecosystem critical):
- Frontend: React 18 + TypeScript
- Main Process: Node.js 20 + Electron Forge
- IPC: ContextBridge with strict channel validation
- Storage: electron-store (encrypted) + SQLite
- Auto-updater: electron-updater (Squirrel)
- Security: CSP strict, contextIsolation: true, sandbox: true
```

### 2.5 Backend & Infrastructure (Shared)
```
Runtime: Node.js 20 LTS or Bun
Framework: Fastify / Express + tRPC or REST
Auth: Supabase Auth / Firebase Auth / Auth0 / Custom JWT
Database: PostgreSQL 16 (primary) + Redis (sessions/cache)
File Storage: Supabase Storage / AWS S3 / Cloudflare R2
Real-time: WebSocket (Socket.io) or Server-Sent Events
Push Gateway: Firebase Cloud Messaging (FCM) + Apple Push Notification Service (APNs)
Queue: BullMQ (Redis) for background jobs
Monitoring: Sentry (crash reporting) + PostHog (analytics) + Firebase Crashlytics
```

---

## SECTION 3: SYSTEM ARCHITECTURE FOR APPLICATIONS

### 3.1 Mobile App Layered Architecture
```
┌─────────────────────────────────────────────────────────────┐
│                    PRESENTATION LAYER                        │
│  Screens → Widgets/Components → Animations → Theme          │
│  Platform Adapters (iOS style vs Material style)            │
├─────────────────────────────────────────────────────────────┤
│                    STATE MANAGEMENT LAYER                    │
│  UI State (Zustand/Riverpod) → Domain State → Sync Engine   │
│  Reactive Streams (RxJS / Dart Streams / Kotlin Flow)       │
├─────────────────────────────────────────────────────────────┤
│                    BUSINESS LOGIC LAYER                      │
│  Use Cases → Interactors → Repositories (abstract)          │
│  Background Workers → Media Processors → ML Inference       │
├─────────────────────────────────────────────────────────────┤
│                    DATA LAYER                                │
│  Local DB (SQLite/Room/CoreData) → Remote API → Cache       │
│  File System → Key-Value Store → Sync Queue                 │
├─────────────────────────────────────────────────────────────┤
│                    PLATFORM LAYER                            │
│  OS APIs (Camera, GPS, Biometrics, Notifications)           │
│  Hardware Abstraction (Sensors, Bluetooth, NFC)             │
│  Background Services (WorkManager / BGTaskScheduler)        │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Offline-First Synchronization Engine
Every app MUST implement:
- **Local Database**: Single source of truth on device
- **Sync Queue**: Outbox pattern for pending changes (create/update/delete)
- **Conflict Resolution**: Last-write-wins (default) OR custom merge strategy
- **Network Awareness**: Pause sync on metered connections (optional user setting)
- **Retry Logic**: Exponential backoff for failed syncs, max 5 retries
- **Delta Sync**: Only sync changed records since last timestamp
- **Background Sync**: iOS BGTaskScheduler, Android WorkManager

### 3.3 Navigation Architecture
**Mobile**:
- Stack Navigator (primary flow)
- Tab Navigator (main sections, max 5 tabs)
- Drawer Navigator (secondary actions, settings)
- Modal presentation (forms, detail views)
- Deep linking: Universal Links (iOS) + App Links (Android)

**Desktop**:
- Multi-window support (macOS) / Single window (Windows default)
- Sidebar navigation (persistent)
- Command palette (Cmd+K / Ctrl+K)
- Tabbed interface within main window
- Drag-and-drop file handling

---

## SECTION 4: DESIGN SYSTEM & PLATFORM COMPLIANCE

### 4.1 Platform Design Guidelines (Strict Adherence)
**iOS (Human Interface Guidelines)**:
- San Francisco font family (system font)
- Dynamic Type support (accessibility text sizes)
- SF Symbols for all icons (vector, 27 weight variations)
- Tab bars at bottom, navigation bars at top
- Sheets for modal presentation (drag to dismiss)
- Context menus on long press (Haptic Touch)
- Safe area insets respected (notch, home indicator, dynamic island)

**Android (Material Design 3)**:
- Roboto / Google Sans font family
- Material Symbols (outlined, rounded, sharp, filled)
- Navigation rail (tablet) or bottom navigation (phone)
- Top app bar with scroll behavior (collapse/expand)
- Floating Action Button (FAB) for primary action
- Snackbars for feedback (not toasts)
- Edge-to-edge layout with window insets

**Desktop (Cross-Platform)**:
- Native title bar or custom frameless (optional)
- Menu bar (macOS) / Hamburger menu (Windows/Linux)
- Keyboard shortcuts (Cmd/Ctrl + key) with shortcut hints
- Right-click context menus
- Resizable window with minimum size constraints
- System tray integration (minimize to tray option)

### 4.2 Visual System (Zero Emoji Policy)
```
Color System:
- Primary: Brand color (500 shade)
- Secondary: Complementary accent
- Surface: Background elevations (0dp, 1dp, 2dp, 3dp, 4dp)
- Error: Destructive actions (#DC2626 or system red)
- Success: Confirmations (#16A34A or system green)
- Warning: Cautions (#EAB308 or system orange)
- Info: Neutral information (#3B82F6 or system blue)
- On-Primary/On-Surface: Text colors with guaranteed contrast

Dark Mode:
- Automatic system detection
- Manual override toggle in settings
- Smooth transition (no flash on launch)
- OLED black option for AMOLED screens (#000000 background)

Typography Scale:
- Display Large: 57sp/64sp (Headlines)
- Display Medium: 45sp/52sp
- Display Small: 36sp/44sp
- Headline Large: 32sp/40sp (Screen titles)
- Headline Medium: 28sp/36sp
- Headline Small: 24sp/32sp
- Title Large: 22sp/28sp (Card titles)
- Title Medium: 16sp/24sp (List items)
- Title Small: 14sp/20sp
- Body Large: 16sp/24sp (Primary reading)
- Body Medium: 14sp/20sp
- Body Small: 12sp/16sp (Captions)
- Label Large: 14sp/20sp (Buttons)
- Label Medium: 12sp/16sp
- Label Small: 11sp/16sp (Overlines)
```

### 4.3 Spacing & Layout Grid
```
Base unit: 8dp (density-independent pixels)
Spacing scale: 4, 8, 12, 16, 20, 24, 32, 40, 48, 64, 80, 96
Corner radius: 0 (sharp), 4, 8, 12, 16, 24, 28 (full for circles)
Elevation (shadows):
- Level 0: 0dp (flat)
- Level 1: 1dp (cards at rest)
- Level 2: 3dp (raised buttons, search bars)
- Level 3: 6dp (floating elements, FAB)
- Level 4: 8dp (dialogs, menus)
- Level 5: 12dp (modals, bottom sheets)
```

### 4.4 Iconography Rules
- **iOS**: SF Symbols (SF Symbol app for browsing), template rendering mode
- **Android**: Material Symbols (outlined default, filled for active states)
- **Cross-platform**: Phosphor Icons (consistent across iOS/Android) or Lucide
- **Desktop**: System native icons where possible, custom set for app-specific
- **NO EMOJI**: Use `checkmark.circle`, `exclamationmark.triangle`, `arrow.right`, `person.crop.circle`, `gearshape`, `bell`, `lock.shield` — never Unicode emoji
- **Size mapping**: 16dp (inline), 20dp (buttons), 24dp (navigation), 32dp (empty states), 48dp (hero icons)

---

## SECTION 5: RESPONSIVE & ADAPTIVE DESIGN

### 5.1 Device Form Factor Support
**Phone (Compact)**:
- Single pane layout
- Bottom navigation or drawer
- Full-screen modals
- Swipe gestures primary navigation
- Touch targets: 48dp x 48dp minimum
- Thumb zone optimization (primary actions at bottom)

**Tablet (Medium/Expanded)**:
- Two-pane master-detail layout
- Navigation rail (side) instead of bottom bar
- Grid layouts (2-3 columns)
- Hover states supported (stylus/mouse)
- Drag and drop between panes

**Desktop / Foldable**:
- Three-pane layout (sidebar + list + detail)
- Resizable split views
- Keyboard navigation (Tab order, arrow keys)
- Context menus on right-click
- Multi-window support (drag content to new window)
- Foldable: Awareness of hinge/fold area (avoid placing content on fold)

### 5.2 Orientation Handling
- **Portrait**: Primary orientation for phones
- **Landscape**: Supported for media viewing, games, tablets
- **Rotation animation**: Smooth transition (300ms)
- **State preservation**: Maintain scroll position, input values across rotation
- **Layout adaptation**: Different constraints for each orientation

### 5.3 Accessibility (Mobile-Specific)
- **VoiceOver (iOS) / TalkBack (Android)**: All interactive elements labeled
- **Dynamic Type**: UI scales with system text size settings (up to 310%)
- **Reduce Motion**: Disable parallax, simplify transitions
- **Reduce Transparency**: Solid backgrounds instead of blur
- **Bold Text**: Heavier font weights system-wide
- **Color Inversion / Grayscale**: Test UI readability
- **Switch Control**: Single-switch navigation compatibility
- **Closed Captions**: All video content captioned

---

## SECTION 6: ANIMATION & INTERACTION SPECIFICATIONS

### 6.1 Motion Principles for Touch Interfaces
- **Immediate feedback**: < 100ms response to touch
- **Physical realism**: Spring animations with damping (not linear)
- **Purposeful**: Guide attention, confirm actions, show relationships
- **Efficient**: 60fps minimum, no dropped frames during transitions

### 6.2 Animation Tokens
```
Durations:
- Instant: 0ms (toggle states)
- Micro: 50ms (color changes, opacity)
- Fast: 150ms (button presses, icon changes)
- Normal: 200-300ms (screen transitions, sheet presentations)
- Slow: 400-500ms (hero animations, page transitions)
- Ambient: 3000ms+ (loading spinners, skeleton shimmer)

Easing:
- Standard: cubic-bezier(0.4, 0.0, 0.2, 1) — Material ease-in-out
- Decelerate: cubic-bezier(0.0, 0.0, 0.2, 1) — Entering elements
- Accelerate: cubic-bezier(0.4, 0.0, 1, 1) — Exiting elements
- Bounce: iOS spring(0.5, 0.7, 0.3) or Flutter Curves.elasticOut
- Linear: Only for continuous loops (spinners, progress)
```

### 6.3 Platform-Specific Transitions
**iOS**:
- Push: Slide from right, current screen slides left and fades slightly
- Modal: Slide from bottom (card presentation on iOS 13+)
- Pop: Reverse of push with parallax
- Tab switch: No animation (instant) or subtle fade

**Android**:
- Activity enter: Slide up + fade in
- Activity exit: Slide down + fade out
- Shared element: Hero transition (image scales and moves to new position)
- Fragment: Slide horizontal with fade
- Bottom sheet: Slide up with scrim fade

**Desktop**:
- Window open: Scale from 0.95 + fade, 200ms
- Window close: Scale to 0.95 + fade, 150ms
- Sidebar collapse: Width animation 300ms, content reflows
- Modal: Center scale + backdrop blur, 200ms
- Toast: Slide from right, auto-dismiss with progress bar

### 6.4 Micro-interactions
- **Button press**: Scale to 0.96, 100ms, spring release
- **Toggle switch**: Thumb slides with color transition, haptic on change
- **Pull to refresh**: Elastic resistance, spinner morphs to checkmark
- **Swipe actions**: Reveal background actions with icon + label
- **Long press**: Scale up slightly (1.02), vibration, context menu appears
- **Error shake**: Horizontal oscillation (translateX -5px to +5px, 3 cycles)
- **Success pulse**: Scale 1.0 → 1.1 → 1.0 with color flash, 400ms

### 6.5 Loading & Empty States
- **Skeleton screens**: Shimmer effect, content structure matches final layout
- **Progressive loading**: Low-res placeholder → high-res image (blur-up)
- **Infinite scroll**: Spinner at bottom, smooth content append
- **Empty state**: Illustration (Lottie or SVG) + headline + CTA button
- **Error state**: Icon + message + retry button (pull to retry)
- **Offline state**: Banner at top, cached content with stale indicator

---

## SECTION 7: COMPONENT ARCHITECTURE

### 7.1 Atomic Design for Mobile
```
src/
├── core/
│   ├── theme/           # Colors, typography, spacing tokens
│   ├── icons/           # Icon registry, SVG assets
│   └── constants/       # App-wide constants
├── components/
│   ├── atoms/           # Text, Button, Input, Icon, Divider
│   ├── molecules/       # SearchBar, FormField, ListTile, Card
│   ├── organisms/       # AppBar, BottomNav, DataTable, Carousel
│   └── templates/       # Screen layouts (Scaffold equivalents)
├── features/
│   ├── auth/            # Login, Register, Biometric, Session
│   ├── profile/         # User profile, Settings, Preferences
│   └── [domain]/        # Feature-specific screens & logic
├── data/
│   ├── models/          # Data classes / structs / interfaces
│   ├── repositories/    # Abstract + Implementation
│   ├── sources/         # Remote API, Local DB, Cache
│   └── sync/            # Sync engine, conflict resolver
├── domain/
│   ├── usecases/        # Business logic operations
│   └── entities/        # Core business objects
├── navigation/
│   ├── router/          # Route definitions, deep links
│   └── guards/          # Auth guards, permission checks
└── utils/
    ├── extensions/      # String, Date, Color extensions
    ├── validators/      # Input validation logic
    └── helpers/         # Formatters, converters
```

### 7.2 Required Component Inventory

**Navigation**:
- Bottom navigation bar (2-5 items, active indicator)
- Top app bar (title, actions, back button, search)
- Navigation rail (tablet/desktop sidebar)
- Drawer / Hamburger menu (secondary navigation)
- Tab bar (segmented control, scrollable tabs)
- Breadcrumbs (desktop only, hierarchical)

**Input**:
- Text field (outlined, filled, underline variants)
- Password field (visibility toggle, strength indicator)
- Search field (clear button, recent searches, suggestions)
- Text area (auto-expand, character counter)
- Dropdown / Picker (native picker on mobile, custom on desktop)
- Date/Time picker (native dialogs, range selection)
- Checkbox / Radio button / Switch / Slider
- Chip / Tag input (create, delete, autocomplete)
- File picker (document, image, multi-select)
- Camera capture (inline preview, retake option)
- Signature pad (canvas-based, export PNG/SVG)

**Data Display**:
- List / Recycler / FlatList (virtualized, 10k+ items)
- Grid (adaptive columns, staggered layout option)
- Card (image, title, subtitle, actions)
- Data table (sortable, filterable, horizontal scroll)
- Chart / Graph (line, bar, pie, sparkline)
- Timeline / Activity feed
- Calendar (month view, agenda view, event markers)
- Map view (annotations, clustering, user location)
- Carousel / PageView (auto-scroll, indicators)
- Image gallery (zoom, pan, swipe between images)
- Video player (controls, fullscreen, PiP, subtitles)
- Audio player (waveform, scrubber, background playback)

**Feedback**:
- Snackbar / Toast (bottom, auto-dismiss, action button)
- Banner (top, persistent, dismissible)
- Dialog / Alert (title, content, actions, critical styling)
- Bottom sheet (modal, persistent, drag handle)
- Progress indicators (linear, circular, determinate, indeterminate)
- Skeleton loading (text, image, card variants)
- Empty state (illustration + text + CTA)
- Error boundary fallback (retry button, report issue)
- Tooltip (desktop hover, mobile long-press or info icon)

**Overlays**:
- Modal (center, full-screen, bottom sheet variants)
- Popover / Menu (anchor to element, scrollable)
- Tooltip (delay 500ms, max 2 lines)
- Bottom sheet (drag handle, snap points: 25%, 50%, 100%)
- Dialog (alert, confirmation, input, full-screen)
- Toast queue (stacking, max 3 visible)

---

## SECTION 8: STATE MANAGEMENT & DATA ARCHITECTURE

### 8.1 Application State Categories
```
Ephemeral State (Local to widget/screen):
- Form field values
- Scroll position
- Animation controllers
- Current tab index

App State (Shared across screens):
- Authentication session
- User preferences (theme, language)
- Navigation stack
- Notification count
- Cart contents (e-commerce)

Server State (Remote source of truth):
- User profile
- Feed content
- Messages
- Settings synced to cloud

Device State (Hardware/OS):
- Network connectivity
- Battery level
- Location permission
- Bluetooth status
```

### 8.2 State Management Patterns
**React Native (Zustand + TanStack Query)**:
```typescript
// Auth store
interface AuthState {
  user: User | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  login: (credentials: LoginDTO) => Promise<void>;
  logout: () => Promise<void>;
  updateProfile: (data: ProfileDTO) => Promise<void>;
}

export const useAuthStore = create<AuthState>((set, get) => ({
  user: null,
  isAuthenticated: false,
  isLoading: false,
  
  login: async (credentials) => {
    set({ isLoading: true });
    try {
      const response = await api.login(credentials);
      await secureStorage.setItem('token', response.token);
      set({ user: response.user, isAuthenticated: true });
    } finally {
      set({ isLoading: false });
    }
  },
  
  logout: async () => {
    await secureStorage.removeItem('token');
    queryClient.clear();
    set({ user: null, isAuthenticated: false });
  }
}));

// Server state hook
export function useFeed(params: FeedParams) {
  return useQuery({
    queryKey: ['feed', params],
    queryFn: () => api.getFeed(params),
    staleTime: 1000 * 60 * 2, // 2 minutes
    gcTime: 1000 * 60 * 10,
  });
}
```

**Flutter (Riverpod)**:
```dart
// Auth notifier
@riverpod
class AuthNotifier extends _$AuthNotifier {
  @override
  AuthState build() => const AuthState.unauthenticated();
  
  Future<void> login(LoginDTO credentials) async {
    state = const AuthState.loading();
    try {
      final response = await ref.read(authRepositoryProvider).login(credentials);
      await ref.read(secureStorageProvider).setToken(response.token);
      state = AuthState.authenticated(response.user);
    } catch (e) {
      state = AuthState.error(e.toString());
    }
  }
}

// Feed provider
@riverpod
Future<List<Post>> feed(FeedRef ref, FeedParams params) async {
  final repository = ref.read(feedRepositoryProvider);
  return repository.getFeed(params);
}
```

### 8.3 Local Persistence Strategy
```
Tier 1 - Secure Storage:
- Auth tokens (Keychain iOS, Keystore Android)
- Biometric enrollment status
- Encryption keys

Tier 2 - Fast Key-Value:
- User preferences (theme, language, onboarding complete)
- Cache timestamps
- Feature flags
- MMKV (RN) / SharedPreferences (Android) / UserDefaults (iOS)

Tier 3 - Structured Database:
- User-generated content (posts, messages, tasks)
- Offline-available reference data
- Sync queue (outbox pattern)
- SQLite (RN) / Room (Android) / CoreData/SwiftData (iOS) / Hive (Flutter)

Tier 4 - File System:
- Images, videos, documents (app documents directory)
- Downloaded content for offline viewing
- Log files (rotated, max 7 days)
```

---

## SECTION 9: NATIVE API INTEGRATION

### 9.1 Permission Management
Implement a centralized permission manager:
```
Permissions to handle:
- Camera (photo/video capture)
- Photo Library (read/write images)
- Location (when in use / always)
- Microphone (voice recording, calls)
- Notifications (local + remote)
- Contacts (read/write)
- Calendar (read/write events)
- Bluetooth (scan, connect, peripheral)
- Health data (iOS HealthKit, Android Health Connect)
- Files / Media (scoped storage on Android 10+)

Flow:
1. Check current status (granted / denied / notDetermined / restricted)
2. If notDetermined → Show rationale dialog (custom, educational)
3. Request system permission
4. If denied → Guide to settings with deep link
5. If permanently denied → Disable feature gracefully, show explanation
```

### 9.2 Camera & Media
- **Camera**: Custom UI overlay on native preview, tap to focus, pinch to zoom, flash control
- **Gallery**: Multi-select, filtering by type (image/video), max selection limit
- **Image Processing**: Background compression (JPEG quality 0.8, max 2048px), thumbnail generation
- **Video**: Trim, compress (H.264/HEVC), max duration enforcement, background upload with progress
- **Document Scan**: Edge detection, perspective correction, B&W filter option

### 9.3 Location Services
- **Accuracy levels**: Navigation (GPS) → Best → 10m → 100m → 3km (power trade-off)
- **Background location**: Geofencing entry/exit events (preferred over continuous tracking)
- **Permission rationale**: Explain why location is needed before system prompt
- **Battery optimization**: Reduce update frequency when stationary, batch updates

### 9.4 Push Notifications
```
Architecture:
- FCM (Android) + APNs (iOS) → Backend gateway → Device
- Local notifications: Scheduled reminders, in-app events
- Rich notifications: Images, action buttons, reply input
- Notification categories: Marketing, transactional, social, system

Implementation:
- Request permission on first relevant action (not at launch)
- Handle foreground notifications (in-app banner vs silent processing)
- Deep link routing from notification tap
- Notification grouping (by conversation, by app section)
- Badge count management (iOS, some Android launchers)
- Notification history (in-app inbox for missed notifications)
```

### 9.5 Biometric Authentication
- **Types**: Face ID, Touch ID, Fingerprint, Iris (device capability detection)
- **Flow**: PIN/Password setup required first → Biometric enrollment → Biometric prompt
- **Fallback**: Always allow PIN/Password fallback, never biometric-only for critical actions
- **Security**: Biometric result not stored, only keychain access granted
- **Use cases**: App unlock, sensitive action confirmation (payments, settings change)

### 9.6 Background Processing
**iOS**:
- Background Fetch: Periodic content updates (30min minimum interval)
- Background Processing: ML inference, data sync, cleanup tasks
- Silent Push Notifications: Trigger background sync
- BGTaskScheduler: Register tasks with constraints (charging, WiFi)

**Android**:
- WorkManager: Guaranteed execution, constraint-aware (network, charging)
- Foreground Service: Continuous location tracking, audio playback
- JobScheduler: API 21+ periodic tasks
- AlarmManager: Exact-time reminders (with Doze mode awareness)

---

## SECTION 10: BACKEND INTEGRATION & APIs

### 10.1 API Client Architecture
```typescript
// Centralized API client with interceptors
class ApiClient {
  private client: AxiosInstance;
  
  constructor() {
    this.client = axios.create({
      baseURL: Config.API_BASE_URL,
      timeout: 30000,
      headers: { 'Content-Type': 'application/json' }
    });
    
    // Request interceptor: Attach auth token
    this.client.interceptors.request.use(async (config) => {
      const token = await secureStorage.getItem('token');
      if (token) config.headers.Authorization = `Bearer ${token}`;
      return config;
    });
    
    // Response interceptor: Token refresh, error handling
    this.client.interceptors.response.use(
      (response) => response,
      async (error) => {
        if (error.response?.status === 401) {
          await refreshToken();
          return this.client.request(error.config);
        }
        return Promise.reject(this.normalizeError(error));
      }
    );
  }
  
  // Upload with progress tracking
  async uploadFile(url: string, file: File, onProgress: (p: number) => void) {
    const formData = new FormData();
    formData.append('file', file);
    return this.client.post(url, formData, {
      headers: { 'Content-Type': 'multipart/form-data' },
      onUploadProgress: (e) => onProgress(e.loaded / e.total)
    });
  }
}
```

### 10.2 Real-Time Features
- **WebSocket**: Connection management, heartbeat (30s ping), auto-reconnect with exponential backoff (max 30s), message queue while offline
- **SSE**: For server-push (lighter than WS, one-way), auto-reconnect
- **Pub/Sub**: Redis-backed for multi-device sync (read status, typing indicators)
- **Presence**: Online/offline status, last seen timestamp, "typing..." indicators

### 10.3 File Upload Strategy
- **Pre-signed URLs**: Backend generates S3/R2 signed URL → App uploads directly → Backend notified on complete
- **Chunked upload**: Large files split into 5MB chunks, parallel upload, resume capability
- **Background upload**: iOS URLSession background task, Android WorkManager + OkHttp
- **Compression**: Images max 2048px, JPEG 80% quality, videos H.264 1080p max
- **Progress tracking**: Per-file percentage, overall queue progress, network-aware pause

---

## SECTION 11: SECURITY HARDENING

### 11.1 Data Protection
- **Encryption at Rest**: SQLCipher for SQLite, Keychain/Keystore for secrets
- **Encryption in Transit**: TLS 1.3 only, certificate pinning for sensitive apps
- **Key Management**: Hardware-backed keystore when available (TEE/SE), fallback to software
- **Memory Safety**: Sensitive data zeroed from memory after use (passwords, tokens)
- **Screenshot Prevention**: `FLAG_SECURE` (Android), `UIApplication.shared.isIdleTimerDisabled` + screen privacy (iOS) for sensitive screens

### 11.2 Authentication & Authorization
- **Token Storage**: Keychain (iOS), EncryptedSharedPreferences (Android), OS keychain (desktop)
- **Token Refresh**: Silent refresh before expiry, queue requests during refresh
- **Session Timeout**: Auto-lock after 5 minutes background, biometric re-prompt
- **Device Binding**: Token tied to device fingerprint (optional high-security mode)
- **Jailbreak/Root Detection**: Warn user, disable sensitive features (optional)
- **Certificate Pinning**: Pin server certificate/hash to prevent MITM attacks

### 11.3 Input Validation & Output Encoding
- **All user input**: Validated on client (UX) AND server (security)
- **SQL Injection**: Impossible with ORM, but raw queries parameterized
- **XSS Prevention**: No inline HTML rendering from API without sanitization
- **Deep Link Validation**: Strict URL scheme validation, reject malformed URLs
- **Intent Validation**: Android intent filters strict, no implicit intent for sensitive actions

### 11.4 Code Obfuscation & Anti-Tampering
- **iOS**: Bitcode disabled (App Store requirement), code stripping
- **Android**: R8/ProGuard obfuscation, native library encryption
- **React Native**: Hermes bytecode, no source maps in production
- **Flutter**: Obfuscation enabled (`--obfuscate --split-debug-info`)
- **Desktop**: ASAR integrity check (Electron), binary signing

---

## SECTION 12: PERFORMANCE OPTIMIZATION

### 12.1 Startup Performance
- **Cold start target**: < 2 seconds to interactive
- **Splash screen**: Branded launch screen (native, not JS-rendered)
- **Lazy loading**: Code split by route, defer non-critical modules
- **Preloading**: Anticipate next screen, preload data and assets
- **Native initialization**: Parallelize SDK initialization (analytics, crash reporting)

### 12.2 Runtime Performance
- **Frame rate**: 60fps minimum, 120fps on ProMotion displays (iOS)
- **JS thread**: Never block for > 100ms, offload to native/workers
- **List virtualization**: Render only visible items + 2 screen buffers (FlashList / RecyclerView / List)
- **Image optimization**: WebP/HEIC format, progressive loading, proper cache sizing
- **Memory management**: Image cache limits (100MB), dispose unused controllers, avoid retain cycles

### 12.3 Bundle Size Optimization
- **Tree shaking**: Remove unused code
- **Asset optimization**: Compress images, use vector graphics (SVG/SF Symbols)
- **Native dependencies**: Audit podfile/build.gradle, remove unused native modules
- **Hermes/V8**: Enable bytecode compilation, disable debugging in release
- **Resource splitting**: Download language packs, assets on demand

### 12.4 Battery & Thermal Optimization
- **Location**: Use significant location changes instead of continuous GPS when possible
- **Network**: Batch API calls, compress payloads (Brotli/gzip), use delta sync
- **Animations**: Pause off-screen animations, reduce frame rate in background
- **Background tasks**: Minimum interval 15 minutes, defer until charging/WiFi
- **Bluetooth**: Scan in low-power mode, disconnect when not needed

---

## SECTION 13: TESTING STRATEGY

### 13.1 Test Pyramid for Apps
```
        /\
       /  \     E2E (Maestro / Detox / Appium) — 5%
      /____\    Critical flows: onboarding → action → logout
     /      \
    /--------\  Integration — 15%
   /          \  API integration, database, native modules
  /------------\
 /              \ Unit — 80%
/________________\ Business logic, utilities, view models
```

### 13.2 Testing Categories
**Unit Tests**:
- Business logic / Use cases (pure functions)
- Data transformation / Formatters
- Validation logic
- State management reducers/notifiers

**Widget / Component Tests**:
- Render tests (does it build without errors?)
- Interaction tests (tap, swipe, input)
- Accessibility tests (labels, roles, focus)

**Integration Tests**:
- API client with mocked server
- Database operations (in-memory SQLite)
- Navigation flow tests
- State management integration

**E2E Tests**:
- Full app flow: Install → Launch → Onboard → Use Feature → Logout
- Critical paths only (too slow for full coverage)
- Real device testing (Firebase Test Lab, AWS Device Farm)
- Visual regression (percy/screenshot tests)

### 13.3 Device Testing Matrix
Minimum device coverage:
- **iOS**: iPhone SE (small screen), iPhone 15 Pro (notch/Dynamic Island), iPhone 15 Pro Max (large), iPad Pro (tablet)
- **Android**: Pixel 7 (standard), Samsung Galaxy S23 (custom skin), Xiaomi Redmi (budget/low-end), Samsung Galaxy Tab (tablet)
- **OS Versions**: Latest - 2 versions (iOS 16/17/18, Android 12/13/14/15)

### 13.4 Live Data Testing
- **Staging environment**: Separate API endpoint, test accounts
- **Feature flags**: Remote config for gradual rollout
- **Beta testing**: TestFlight (iOS), Google Play Internal Testing / Closed Testing
- **Dogfooding**: Team uses app daily with staging backend
- **Crash monitoring**: Firebase Crashlytics, Sentry for JS errors
- **Performance monitoring**: Firebase Performance, custom timing logs

---

## SECTION 14: BUILD, SIGNING & DISTRIBUTION

### 14.1 Build Configuration
**iOS**:
- Xcode project with schemes: Debug, Staging, Release
- Build settings: Optimization level, strip debug symbols, enable bitcode (NO)
- Capabilities: Push Notifications, Background Modes, Keychain Sharing, Associated Domains
- Provisioning profiles: Development, Ad Hoc, App Store
- Code signing: Automatic with Apple ID or manual with certificates

**Android**:
- Build variants: debug, staging, release
- Signing config: Debug (auto), Release (keystore file, env variables)
- ProGuard/R8: Enabled for release, mapping file generated
- App Bundles: AAB format for Play Store (not APK)
- Target/Compile SDK: Latest stable

**Desktop**:
- Code signing certificate: EV cert for Windows (SmartScreen), Apple Developer ID for macOS
- Notarization: macOS required (staple notarization ticket)
- Installer: MSI (Windows), DMG (macOS), AppImage + DEB/RPM (Linux)
- Auto-updater: Delta updates, signature verification

### 14.2 CI/CD Pipeline
```yaml
# GitHub Actions for Mobile
jobs:
  lint-and-typecheck:
    runs-on: ubuntu-latest
    steps:
      - checkout
      - setup-node / setup-flutter
      - install dependencies
      - run lint
      - run type check / static analysis
      - run unit tests

  build-ios:
    runs-on: macos-latest
    needs: lint-and-typecheck
    steps:
      - checkout
      - setup Ruby / CocoaPods
      - install certificates (match / fastlane)
      - build archive
      - upload to TestFlight (fastlane pilot)

  build-android:
    runs-on: ubuntu-latest
    needs: lint-and-typecheck
    steps:
      - checkout
      - setup JDK 17
      - decode keystore from secrets
      - build AAB
      - upload to Play Console Internal Testing

  e2e-tests:
    runs-on: macos-latest
    needs: [build-ios, build-android]
    steps:
      - checkout
      - run Maestro / Detox tests
      - upload artifacts and screenshots
```

### 14.3 App Store Optimization (ASO)
- **App Name**: 30 characters max (iOS), 50 characters (Android) — keyword optimized
- **Subtitle**: Brief value proposition
- **Description**: Structured with features, social proof, call-to-action
- **Keywords**: 100 character field (iOS only), research with AppTweak/SensorTower
- **Screenshots**: 5-10 screenshots per device size, localized
- **Preview Video**: 15-30 seconds, auto-play muted
- **Ratings**: In-app review prompt (SKStoreReviewController / In-App Review API)
- **Localization**: App store metadata + app content in top 5 markets

---

## SECTION 15: DEPLOYMENT & OPERATIONS

### 15.1 Release Management
- **Versioning**: Semantic versioning (MAJOR.MINOR.PATCH)
- **Release notes**: User-facing changelog per version
- **Phased rollout**: 5% → 20% → 50% → 100% over 7 days
- **Rollback plan**: Previous version promotion within 1 hour
- **Hotfix policy**: Critical bugs = immediate release, bypass normal queue

### 15.2 Monitoring & Observability
- **Crash Reporting**: Firebase Crashlytics (primary), Sentry (JS layer)
- **Analytics**: Mixpanel / Amplitude (product), Firebase Analytics (baseline)
- **Performance**: Firebase Performance Monitoring (startup, screen render, API latency)
- **Logging**: Structured logs, 7-day retention, no PII in logs
- **User Feedback**: In-app shake-to-report, screenshot annotation, feedback form

### 15.3 Update Strategy
- **Forced updates**: Block app usage if API incompatible (major version mismatch)
- **Soft updates**: Banner prompting update for bug fixes
- **Silent updates**: CodePush (RN) / Shorebird (Flutter) for JS/Dart layer hotfixes
- **Deprecation windows**: 2 versions supported by backend API

---

## SECTION 16: DOCUMENTATION REQUIREMENTS

### 16.1 README.md Structure
```markdown
# App Name

[![iOS](https://img.shields.io/badge/App_Store-Download-blue)](link)
[![Android](https://img.shields.io/badge/Play_Store-Download-green)](link)
[![CI](https://github.com/user/repo/actions/workflows/ci.yml/badge.svg)](link)

> One-line description of the app.

[App Store](link) · [Play Store](link) · [Website](link) · [Documentation](link)

## Features

- [x] Feature one with icon (CheckCircle)
- [x] Feature two
- [x] Feature three

## Screenshots

| iOS | Android | Desktop |
|-----|---------|---------|
| img | img     | img     |

## Tech Stack

- **Mobile**: React Native 0.74 / Flutter 3.22
- **State**: Zustand / Riverpod
- **Backend**: Node.js + tRPC + PostgreSQL
- **Auth**: Supabase Auth
- **Storage**: Supabase Storage

## Quick Start

```bash
# 1. Clone
git clone https://github.com/user/repo.git
cd repo

# 2. Install
npm install / flutter pub get
cd ios && pod install

# 3. Environment
cp .env.example .env

# 4. Run
npx react-native run-ios / flutter run
```

## Architecture

[Insert architecture diagram]

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md)

## License

MIT (c) [Your Name]
```

### 16.2 Additional Documentation
- `ARCHITECTURE.md` — Module structure, data flow, state management
- `NATIVE_MODULES.md` — Custom native module integration guide
- `RELEASE.md` — Build, sign, and release checklist
- `TESTING.md` — How to run tests, device requirements
- `docs/API.md` — Mobile API contract (endpoints, payloads, auth)
- `docs/DEPLOYMENT.md` — CI/CD setup, store publishing steps
- `PRIVACY.md` — Data collection practices, third-party SDKs

---

## SECTION 17: PROJECT-SPECIFIC ADAPTATIONS

### 17.1 Social / Messaging Apps
Additional requirements:
- Real-time messaging (WebSocket with message queue)
- End-to-end encryption (Signal Protocol or similar)
- Media sharing (image/video/document with compression)
- Voice/Video calls (WebRTC integration)
- Push notifications for messages (rich with sender info)
- Message statuses: sending → sent → delivered → read
- Typing indicators and presence
- Group chats (roles: admin, moderator, member)
- Message reactions, replies, threads
- Content moderation (auto-filter, report system)

### 17.2 E-Commerce / Marketplace Apps
Additional requirements:
- Product catalog with filters (category, price, rating)
- Cart persistence (local + server sync)
- Checkout flow (Stripe / PayPal / Apple Pay / Google Pay)
- Order tracking with push notifications
- Wishlist / Favorites
- Barcode/QR scanner for product lookup
- AR product preview (ARKit / ARCore)
- Reviews and ratings with photo upload
- Inventory management (real-time stock)
- Returns and refunds workflow

### 17.3 Fitness / Health Apps
Additional requirements:
- HealthKit (iOS) / Health Connect (Android) integration
- Activity tracking (steps, distance, calories)
- Workout recording (GPS trace, heart rate, splits)
- Background location for route tracking
- Sensor integration (accelerometer, gyroscope, barometer)
- Data visualization (charts for progress over time)
- Goal setting and streak tracking
- Social features (challenges, leaderboards)
- Wearable support (Apple Watch, Wear OS)

### 17.4 Fintech / Banking Apps
Additional requirements:
- Biometric authentication mandatory
- Certificate pinning on all API calls
- Screenshot prevention on sensitive screens
- Jailbreak/root detection
- Transaction history with search and filters
- Push notifications for transactions
- Budgeting and spending analytics
- Bill pay and transfer workflows
- Regulatory compliance (PCI-DSS, SOC-2)
- Offline transaction queue (sync when online)

### 17.5 Productivity / Enterprise Apps
Additional requirements:
- Offline-first document editing
- Real-time collaboration (Operational Transform / CRDT)
- Calendar integration (Google / Outlook / Exchange)
- File sync (like Dropbox, delta sync)
- Task management with assignments
- Team collaboration (comments, mentions)
- Admin dashboard (user management, analytics)
- SSO integration (SAML, OIDC)
- MDM support (AppConfig, managed configurations)

---

## SECTION 18: FINAL DELIVERY CHECKLIST

### Code Quality
- [ ] TypeScript strict mode / Dart strong mode, zero dynamic/any types
- [ ] Linting passes (ESLint/Clippy/ktlint/detekt) with zero warnings
- [ ] No console.log / print statements in production
- [ ] Minimum 10,000 lines of production code (excluding generated, tests, assets)
- [ ] No emoji characters anywhere — verified with grep/regex
- [ ] All TODOs resolved or ticketed

### Platform Compliance
- [ ] iOS: Builds for iPhone + iPad, iOS 15+ support, no deprecated APIs
- [ ] Android: Builds for phone + tablet, API 26+, scoped storage compliant
- [ ] Desktop: Signed binaries, auto-updater tested, system tray works
- [ ] App Store / Play Store metadata complete (screenshots, description, privacy)

### Functionality
- [ ] All features from requirements implemented
- [ ] Offline mode works (airplane mode test)
- [ ] Deep linking functional (universal links + app links)
- [ ] Push notifications received in foreground and background
- [ ] Biometric auth works with fallback to PIN
- [ ] All permissions handled with rationale dialogs
- [ ] Background tasks execute reliably
- [ ] App state restoration (kill app, relaunch → restore position)

### Performance
- [ ] Cold start < 2 seconds on mid-tier device
- [ ] 60fps maintained during list scrolling
- [ ] Memory usage stable (no leaks after 30 min usage)
- [ ] Battery drain < 5% per hour for active use
- [ ] App size < 50MB (excluding assets) for mobile

### Security
- [ ] Auth tokens stored in secure storage (Keychain/Keystore)
- [ ] Certificate pinning active (if required)
- [ ] No hardcoded secrets in source code
- [ ] ProGuard/R8 obfuscation enabled
- [ ] Screenshot prevention on sensitive screens (if required)
- [ ] Input validation on all forms

### Testing
- [ ] Unit test coverage > 70%
- [ ] Integration tests for API and database layers
- [ ] E2E tests for 3 critical user journeys
- [ ] Tested on physical devices (not just simulators)
- [ ] Beta testing completed (TestFlight + Play Internal Testing)

### DevOps
- [ ] CI/CD pipeline green
- [ ] Automated builds for iOS and Android
- [ ] Staging deployment tested
- [ ] Crash reporting configured and tested
- [ ] Analytics events firing correctly

### Documentation
- [ ] README with quick-start and screenshots
- [ ] Architecture diagram included
- [ ] API documentation for mobile endpoints
- [ ] Release checklist documented
- [ ] Privacy policy URL configured
- [ ] Changelog updated

---

## MANDATORY OUTPUT SEQUENCE

When generating a mobile/desktop app for a user, produce deliverables in this order:

1. **Analysis** — Platform selection, user personas, hardware requirements, offline strategy
2. **Architecture** — Layered diagram, state management plan, sync engine design, navigation flow
3. **Design System** — Platform-specific tokens (iOS HIG / Material 3), typography, spacing, icons
4. **Project Structure** — Complete folder tree with module purposes
5. **Core Implementation** — Config, native setup, navigation, state management, API client
6. **Feature Implementation** — Screen-by-screen, feature-by-feature code
7. **Native Integration** — Permissions, camera, location, push notifications, biometrics
8. **Backend Integration** — API contracts, sync logic, real-time features
9. **Testing Suite** — Unit, widget, integration, and E2E test files
10. **Build & Deploy** — CI/CD, signing config, store metadata, release checklist
11. **Documentation** — README, architecture docs, API reference, privacy policy

**REMEMBER**:
- NO EMOJIS — use SF Symbols, Material Icons, Phosphor, or Lucide
- MINIMUM 10,000 lines of production-grade code
- EVERY instruction above must be visibly satisfied
- Platform guidelines are NON-NEGOTIABLE (Apple will reject non-compliant apps)
- When in doubt, over-engineer rather than under-deliver

---

*End of GOVINDA CLI Mobile & Desktop App Protocol v3.0*