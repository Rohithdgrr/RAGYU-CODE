 # GOVINDA CLI — BROWSER EXTENSION, BOTS, CLI TOOLS & API SERVICES DEVELOPMENT PROTOCOL
## Version 5.0 | Specialized Blueprint for Extensions, Automations, Command-Line Interfaces & Backend Services

---

## PART A: BROWSER EXTENSION DEVELOPMENT PROTOCOL

### SECTION A1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Extension Type**: Content script injector / Popup utility / Background service / DevTools panel / New Tab override / Side panel / Omnibox provider
- **Target Browsers**: Chrome (Manifest V3) / Firefox (MV3 with V2 fallback) / Edge / Safari (native App Extension) / Opera
- **Permission Model**: Minimal required, optional permissions pattern, host permissions scope
- **Content Access**: Specific domains (`<all_urls>`, `activeTab`, specific sites), page action vs browser action
- **Cross-Origin Requirements**: External API calls, third-party script injection, iframe communication

**Input / Output Matrix**:

| Input Channel | Format | Validation | Handling |
|--------------|--------|------------|----------|
| Browser tabs | Tab ID, URL, title | URL pattern matching | Content script targeting |
| Web page DOM | HTML / CSS / JS runtime | MutationObserver, selectors | Sandboxed content script |
| User interactions | Click, keyboard, context menu | Event delegation | Message to background |
| Browser storage | Key-value pairs | Schema validation | chrome.storage API |
| External APIs | JSON / XML / binary | CORS headers, CSP | Background fetch |
| Context menus | Click event, page info | Menu item ID validation | Command routing |
| Omnibox | Text input | Regex, suggestion limits | Async suggestion provider |

| Output Channel | Format | Optimization | Fallback |
|---------------|--------|--------------|----------|
| DOM injection | HTML / CSS / JS | Shadow DOM isolation, CSS scoping | Disabled if CSP blocks |
| Browser notifications | Rich notification | Action buttons, icons | In-page toast fallback |
| Badge text | 4 characters max | Color coding (red/green/blue) | Tooltip fallback |
| Side panel / Popup | React/Vue UI | Lazy load, code split | Minimal HTML fallback |
| DevTools panel | Custom UI | Performance conscious | Console API fallback |
| File download | Blob / Data URL | Stream large files | Chunked download |
| Clipboard | Text / HTML / image | Permissions API | Manual copy fallback |

**Working Mechanism**:
1. **Lifecycle**: Install → Update → Browser startup → Page load → Content script inject → User interaction → Background processing → Storage update → UI render
2. **Process Isolation**: Background service worker (ephemeral, event-driven) + Content script (isolated world per frame) + Popup (temporary, on-demand) + Options page (persistent)
3. **Message Passing**: One-time messages (`chrome.runtime.sendMessage`) + Long-lived ports (`chrome.runtime.connect`) + Native messaging (host app)
4. **State Sync**: `chrome.storage.local` (unlimited, async) + `chrome.storage.sync` (cross-device, quota-limited) + `chrome.storage.session` (MV3, memory-only)
5. **Content Script Injection**: Static (manifest-declared) + Dynamic (`scripting.executeScript`) + CSS injection (`scripting.insertCSS`)

---

### SECTION A2: TECH-STACK MANDATE

```
Core Framework: Vanilla TypeScript (preferred) or React 18+ / Vue 3 (for complex UI)
Build Tool: Vite (extension mode) / WXT (modern web extension framework) / Plasmo
Language: TypeScript 5.x (strict), ES2022 target
Styling: Shadow DOM + CSS Modules / Tailwind (with shadow DOM scoping)
State: chrome.storage API wrapper with reactive subscriptions
Icons: Lucide (SVG, inlined) / Phosphor — NO EMOJI CHARACTERS
Package Manager: pnpm (workspace support for multi-extension repos)

Browser Polyfills: webextension-polyfill (unified API across browsers)
Testing: Vitest (unit) + Playwright (E2E with loaded extension)
Linting: ESLint + webextensions ESLint plugin
```

**Browser-Specific Adaptations**:
- **Chrome**: Manifest V3, service worker background, `chrome` namespace
- **Firefox**: Manifest V2/V3 hybrid, optional background page (persistent allowed), `browser` namespace with Promise support
- **Safari**: Native Xcode project, `SFSafariApplication`, App Extension bundle, macOS app wrapper required
- **Edge**: Chromium-based, identical to Chrome, Edge Add-ons store
- **Cross-browser build**: WXT or custom build pipeline generating per-browser manifests

---

### SECTION A3: SYSTEM ARCHITECTURE

```
┌─────────────────────────────────────────────────────────────┐
│                    POPUP / OPTIONS UI                        │
│  React/Vue components → Shadow DOM → Isolated styles         │
│  Temporary lifecycle (opens → loads → closes → unloads)     │
├─────────────────────────────────────────────────────────────┤
│                    BACKGROUND SERVICE WORKER                 │
│  Event-driven, ephemeral, stateless between events           │
│  Message router → API client → Storage manager → Alarm jobs  │
│  Offscreen document (Chrome 109+ for DOM APIs in SW)        │
├─────────────────────────────────────────────────────────────┤
│                    CONTENT SCRIPTS                           │
│  Isolated world (no JS conflicts with page)                  │
│  DOM observer → Element picker → Message relay → CSS inject  │
│  Main world injection (optional, for page JS interop)        │
├─────────────────────────────────────────────────────────────┤
│                    INJECTED SCRIPTS (MAIN WORLD)             │
│  Runs in page JS context (shared window object)              │
│  Communication via CustomEvent / window.postMessage          │
│  Used for page script interop, not DOM manipulation          │
├─────────────────────────────────────────────────────────────┤
│                    DEVTOOLS / SIDE PANEL                     │
│  Panel UI → inspectedWindow.eval → Resource monitoring       │
└─────────────────────────────────────────────────────────────┘
```

**Communication Patterns**:
```typescript
// Typed message protocol
interface ExtensionMessage {
  type: 'FETCH_DATA' | 'UPDATE_UI' | 'EXECUTE_SCRIPT' | 'STORAGE_CHANGE';
  payload: unknown;
  timestamp: number;
  source: 'popup' | 'content' | 'background' | 'options';
}

// One-time message
chrome.runtime.sendMessage({ type: 'FETCH_DATA', payload: { url } });

// Long-lived port (for streaming data)
const port = chrome.runtime.connect({ name: 'stream-port' });
port.postMessage({ type: 'SUBSCRIBE', topic: 'notifications' });
port.onMessage.addListener((msg) => { /* handle */ });
```

---

### SECTION A4: MANIFEST V3 ARCHITECTURE (Non-Negotiable)

```json
{
  "manifest_version": 3,
  "name": "Extension Name",
  "version": "1.0.0",
  "description": "Concise description for store listing",
  "permissions": [
    "storage",
    "activeTab",
    "scripting",
    "alarms",
    "notifications"
  ],
  "optional_permissions": [
    "history",
    "bookmarks",
    "clipboardWrite"
  ],
  "host_permissions": [
    "https://example.com/*",
    "https://api.service.com/*"
  ],
  "optional_host_permissions": [
    "<all_urls>"
  ],
  "background": {
    "service_worker": "background.js",
    "type": "module"
  },
  "content_scripts": [
    {
      "matches": ["https://example.com/*"],
      "js": ["content.js"],
      "css": ["content.css"],
      "run_at": "document_idle",
      "world": "ISOLATED"
    }
  ],
  "action": {
    "default_popup": "popup.html",
    "default_icon": {
      "16": "icons/icon16.png",
      "32": "icons/icon32.png",
      "48": "icons/icon48.png",
      "128": "icons/icon128.png"
    },
    "default_title": "Click to open"
  },
  "options_page": "options.html",
  "options_ui": {
    "page": "options.html",
    "open_in_tab": true
  },
  "icons": {
    "16": "icons/icon16.png",
    "48": "icons/icon48.png",
    "128": "icons/icon128.png"
  },
  "web_accessible_resources": [
    {
      "resources": ["assets/inject.js", "assets/style.css"],
      "matches": ["https://example.com/*"]
    }
  ],
  "content_security_policy": {
    "extension_pages": "script-src 'self'; object-src 'self'; connect-src 'self' https://api.service.com;"
  },
  "externally_connectable": {
    "matches": ["https://example.com/*"]
  }
}
```

**Icon Requirements**:
- Sizes: 16x16 (favicon/toolbar), 32x32 (Windows HiDPI), 48x48 (Chrome Web Store), 128x128 (store listing)
- Formats: PNG with transparency, SVG (Chrome/Edge only, preferred for scaling)
- Variants: Color for light mode, white/monochrome for dark mode toolbar
- **NO EMOJI**: Use geometric vector icons, flat design, 1-2 color maximum

---

### SECTION A5: DESIGN SYSTEM FOR EXTENSIONS

**Visual Identity**:
- **Width constraints**: Popup max 800x600px (Chrome limit), options page full tab
- **Theme adaptation**: Detect browser theme (`chrome.theme.onUpdated`), match light/dark
- **Typography**: System font stack (`system-ui, -apple-system, sans-serif`), 14px base
- **Spacing**: 8px base grid, compact density (extensions have limited space)
- **Colors**: Respect browser accent color, neutral grays for chrome, semantic colors for status

**Popup UI Standards**:
- Max height: 600px before scroll
- Header: Icon + Title + Close/Settings button
- Content: Scrollable area with padding
- Footer: Status indicator + version number
- Loading states: Skeleton inline, not overlay (space constrained)
- Empty states: Centered icon + text + action button

**Content Script UI Standards**:
- **Shadow DOM mandatory**: Prevents CSS leakage to/from host page
- **CSS reset**: Scoped to shadow root only
- **Positioning**: Fixed overlay, avoid `z-index` wars (use 999999 if necessary)
- **Dismissal**: ESC key, click outside, auto-dismiss after timeout
- **Accessibility**: Injected elements must be keyboard navigable, screen reader friendly

---

### SECTION A6: SECURITY & PRIVACY

**Content Security Policy (Strict)**:
- `script-src 'self'` — NO inline scripts, NO `eval()`, NO `Function()`
- `object-src 'self'` — No external objects
- `connect-src` — Whitelist API domains explicitly
- No remote code execution: All code bundled at build time
- No external scripts: CDN scripts forbidden, bundle everything

**Permission Minimization**:
- Use `activeTab` instead of `<all_urls>` where possible (temporary access)
- Request optional permissions at runtime (`chrome.permissions.request`)
- Host permissions: Narrowest match (`https://site.com/path/*` not `*://*/*`)
- Justification: Store listing must explain why each permission is needed

**Data Handling**:
- No storage of PII without encryption
- `chrome.storage.local` data cleared on uninstall option
- No tracking pixels, no analytics without opt-in
- Secure messaging: Validate message sender (`sender.id === chrome.runtime.id`)

**Cross-Site Scripting Prevention**:
- Sanitize all DOM insertions (DOMPurify bundled)
- Use `textContent` instead of `innerHTML` where possible
- Validate all message payloads with Zod schemas
- Content script isolation prevents direct page JS access

---

### SECTION A7: PERFORMANCE & OPTIMIZATION

**Service Worker Lifecycle**:
- SW terminates after 30 seconds of inactivity (Chrome) — design for event-driven wake
- Use `chrome.alarms` instead of `setInterval` for periodic tasks (persists across SW restarts)
- Offscreen Document (Chrome 109+): For DOM-dependent APIs (clipboard, DOMParser) in SW context
- Keep SW startup < 200ms — defer non-critical initialization

**Content Script Performance**:
- Lazy inject: Use `document_idle` (default) or `document_end`, avoid `document_start` unless necessary
- MutationObserver: Debounce mutations, disconnect when not needed
- CSS injection: Minimal rules, avoid `!important` wars with host page
- Memory: Cleanup event listeners on navigation (SPAs don't always reload)

**Bundle Optimization**:
- Tree shake aggressively (extensions have memory limits)
- Code split by entry point (popup, content, background separate chunks)
- Asset optimization: Icons as SVG, not PNG where supported
- Total extension size: < 10MB for Chrome Web Store, < 200MB for Firefox

---

### SECTION A8: TESTING STRATEGY

**Unit Tests**:
- Mock `chrome.*` APIs with sinon-chrome or custom mocks
- Test message handlers, storage operations, utility functions
- Test content script DOM utilities in JSDOM

**Integration Tests**:
- Message passing between contexts (popup ↔ background ↔ content)
- Storage sync across contexts
- API client with mocked fetch

**E2E Tests**:
- Playwright with extension loaded (`--load-extension` flag)
- Test on real websites (local test server or controlled domain)
- Verify DOM injection doesn't break host page
- Cross-browser testing: Chrome + Firefox minimum

**Manual Testing Checklist**:
- [ ] Install/uninstall flow clean
- [ ] Update from previous version preserves data
- [ ] Permissions prompt appears at correct time
- [ ] Popup renders correctly on all screen sizes
- [ ] Content script doesn't break SPA navigation (React Router, Vue Router)
- [ ] Background SW restarts and recovers state
- [ ] Storage quota not exceeded
- [ ] CSP blocks inline script attempts

---

### SECTION A9: DISTRIBUTION & DEPLOYMENT

**Chrome Web Store**:
- ZIP upload (manifest at root)
- Store listing: 1280x800 or 640x400 screenshots, 440x280 marquee tile
- Privacy practices declaration: Data usage disclosure mandatory
- Review time: 1-3 days for simple extensions, longer for sensitive permissions
- Payment: One-time or subscription via Chrome Web Store Payments (or external)

**Firefox Add-ons (AMO)**:
- ZIP or XPI format
- Review: Manual code review, stricter privacy requirements
- `browser_specific_settings` in manifest for Firefox-only features
- Optional permissions: `optional_permissions` array

**Edge Add-ons**:
- Identical to Chrome, separate store submission
- Microsoft Partner Center account required

**Safari App Extension**:
- Requires macOS app wrapper (Xcode project)
- Distributed via Mac App Store or direct download
- Native messaging host app optional

**Self-Distribution**:
- CRX (Chrome, enterprise policy install)
- XPI signed by Mozilla (unlisted)
- Update URL in manifest for auto-update (`update_url` field)

---

### SECTION A10: EXTENSION-SPECIFIC ADAPTATIONS

**Content Blocker / Privacy Extension**:
- Declarative Net Request API (MV3) — static ruleset + dynamic rules
- Rule limits: 30,000 static + 5,000 dynamic (Chrome), 300,000 (Firefox)
- DNR vs webRequest API: DNR preferred in MV3, webRequest blocking deprecated
- Filter lists: EasyList format parsing, update mechanism

**Password Manager / Form Filler**:
- Secure storage: `chrome.storage.local` encrypted with user master password
- Autofill: Detect form fields by heuristics (type, name, id, aria-label)
- In-field UI: Shadow DOM icon button inside password fields
- Biometric unlock: Native messaging to host app for OS biometric API

**Developer Tools Extension**:
- DevTools API: `chrome.devtools.panels.create()`
- Eval in inspected window: `chrome.devtools.inspectedWindow.eval()`
- Network monitoring: `chrome.devtools.network.onRequestFinished`
- Panel UI: React/Vue in devtools panel context

**AI / LLM Assistant Extension**:
- Side panel API (`chrome.sidePanel`) for persistent AI chat
- Content script: Text selection popup ("Ask AI about this")
- Streaming responses: Server-Sent Events from background service worker
- Context injection: Summarize page, answer questions about content

---

### SECTION A11: FINAL CHECKLIST

- [ ] Manifest V3 compliant (or V2 for Firefox with V3 migration plan)
- [ ] All permissions justified in store listing
- [ ] CSP strict, no inline scripts, no eval
- [ ] Content script uses Shadow DOM for UI injection
- [ ] Background service worker restarts gracefully (no persistent state assumptions)
- [ ] Cross-browser compatibility: Chrome + Firefox tested
- [ ] Icons provided in all required sizes (16, 32, 48, 128px)
- [ ] Screenshots for store listing (1280x800, 640x400)
- [ ] Privacy policy URL provided
- [ ] No emoji in UI — vector icons only
- [ ] Minimum 10,000 lines if building full extension framework/platform

---

## PART B: BOT DEVELOPMENT PROTOCOL

### SECTION B1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Platform**: Discord / Slack / Telegram / WhatsApp / Microsoft Teams / Custom webhook
- **Bot Type**: Command bot / Event responder / Workflow automation / AI assistant / Moderation / Notification relay
- **Interaction Model**: Slash commands / Prefix commands / Mention triggers / Button interactions / Modal forms / Context menus / Scheduled tasks / Event-driven reactions
- **Scale**: Single server/guild / Multi-tenant (thousands of servers) / Enterprise (internal workspace)
- **Latency Requirement**: Real-time chat (< 100ms) / Async tasks (< 5s acceptable)

**Input / Output Matrix**:

| Input Channel | Format | Validation | Handling |
|--------------|--------|------------|----------|
| Chat messages | Text + embeds + attachments | Rate limit check, spam filter | Command parser, NLP pipeline |
| Slash commands | Structured JSON interaction | Permission check, option validation | Command router, deferred reply |
| Button clicks | Custom ID + metadata | Session validation, timeout check | Interaction handler |
| Modal submits | Form field values | Schema validation (Zod) | Data processing, confirmation |
| Select menus | String/entity values | Option existence check | State update, navigation |
| Scheduled events | Cron trigger / timestamp | Job deduplication, timezone | Background worker execution |
| External APIs | JSON / XML / webhook | Signature verification (HMAC) | Event emitter, queue |
| File uploads | Binary stream | MIME type, size limit, virus scan | Storage, processing queue |

| Output Channel | Format | Optimization | Fallback |
|---------------|--------|--------------|----------|
| Chat replies | Text / embeds / components | Message splitting (>2000 chars) | DM user if permission denied |
| Direct messages | Private channel | Rate limit respect | Email notification fallback |
| Ephemeral messages | Visible only to user | Immediate feedback | Edit previous ephemeral |
| Webhook responses | JSON interaction response | Deferred reply for >3s tasks | Follow-up message |
| Reactions | Unicode emoji / custom | Permission check | Text acknowledgment fallback |
| Threads | Thread creation + reply | Auto-archive setting | Channel message fallback |
| External actions | API call / DB write / file write | Background queue | Retry with exponential backoff |
| Voice audio | Opus stream | Bitrate adaptation | Text fallback if voice fails |

---

### SECTION B2: TECH-STACK MANDATE

**Discord**:
```
Runtime: Node.js 20 LTS
Framework: discord.js v14+ (recommended) or Eris
Language: TypeScript 5.x (strict)
State: Redis (sessions, cooldowns) + PostgreSQL (persistent data)
Queue: BullMQ (background jobs) + Redis Pub/Sub (shard communication)
Monitoring: Sentry (errors) + Prometheus (metrics)
Hosting: Docker + VPS / Railway / Fly.io
```

**Slack**:
```
Runtime: Node.js 20 or Python 3.11+
Framework: Bolt for JavaScript / Python (official)
API: Slack Web API + Socket Mode (websocket) or HTTP Event Adapter
Storage: PostgreSQL + Redis (stateless bot instances)
Hosting: AWS Lambda (serverless) or containerized
```

**Telegram**:
```
Runtime: Node.js 20 or Python 3.11+
Framework: node-telegram-bot-api / Telegraf (Node) or python-telegram-bot / aiogram (Python)
Mode: Webhook (production) or Long Polling (development)
Storage: PostgreSQL + Redis
Hosting: Any VPS, webhook requires HTTPS + domain
```

**WhatsApp Business**:
```
API: WhatsApp Business API (Meta) via Twilio or direct
Framework: Custom HTTP handler + webhook verification
Storage: PostgreSQL for conversation state
Compliance: Opt-in required, 24-hour session window, template messages
```

**Cross-Platform (Multi-Platform Bot)**:
```
Abstraction: Custom adapter pattern per platform
Core: Platform-agnostic business logic
Adapters: DiscordAdapter, SlackAdapter, TelegramAdapter
Unified API: sendMessage(), createThread(), addReaction(), etc.
```

---

### SECTION B3: SYSTEM ARCHITECTURE

```
┌─────────────────────────────────────────────────────────────┐
│                    GATEWAY / ADAPTER LAYER                   │
│  Discord Gateway (WebSocket) / Slack Socket Mode /           │
│  Telegram Webhook / WhatsApp Webhook                         │
│  Rate limiter → Connection manager → Heartbeat handler       │
├─────────────────────────────────────────────────────────────┤
│                    EVENT PROCESSING LAYER                    │
│  Event parser → Middleware chain (auth, spam, logging)       │
│  Command router → Interaction handler → Component handler    │
│  Natural Language Parser (optional AI layer)                 │
├─────────────────────────────────────────────────────────────┤
│                    BUSINESS LOGIC LAYER                      │
│  Command implementations → Services → Use cases              │
│  Conversation manager → Context tracker → Memory store       │
│  Scheduled jobs → Background workers → External API clients  │
├─────────────────────────────────────────────────────────────┤
│                    DATA LAYER                                │
│  PostgreSQL (guild configs, user data, logs)                │
│  Redis (sessions, cooldowns, rate limits, job queues)       │
│  Vector DB (conversation memory, semantic search)            │
│  S3 (file attachments, exports)                            │
├─────────────────────────────────────────────────────────────┤
│                    EXTERNAL INTEGRATIONS                     │
│  AI/LLM APIs (OpenAI, Anthropic, local)                    │
│  Third-party APIs (GitHub, Jira, Stripe, etc.)             │
│  Webhook receivers (GitHub Actions, CI/CD, monitoring)     │
└─────────────────────────────────────────────────────────────┘
```

**Sharding Architecture** (Discord large bots):
- Discord limits: 2500 guilds per websocket connection
- Shard manager: Spawns child processes per shard
- Inter-process communication: Redis Pub/Sub or IPC
- Shared state: Redis cluster (not local memory)
- Zero-downtime deploy: Rolling restart per shard

---

### SECTION B4: COMMAND & INTERACTION ARCHITECTURE

**Discord Slash Commands**:
```typescript
// Command definition
const command = new SlashCommandBuilder()
  .setName('task')
  .setDescription('Manage tasks')
  .addSubcommand(sub => 
    sub.setName('create')
      .setDescription('Create a new task')
      .addStringOption(opt => 
        opt.setName('title').setDescription('Task title').setRequired(true)
      )
      .addUserOption(opt =>
        opt.setName('assignee').setDescription('Who to assign').setRequired(false)
      )
  )
  .setDefaultMemberPermissions(PermissionFlagsBits.ManageMessages)
  .setDMPermission(false);

// Handler
async function execute(interaction: ChatInputCommandInteraction) {
  await interaction.deferReply({ ephemeral: true });
  
  const title = interaction.options.getString('title', true);
  const assignee = interaction.options.getUser('assignee');
  
  try {
    const task = await taskService.create({ title, assigneeId: assignee?.id });
    const embed = new EmbedBuilder()
      .setTitle('Task Created')
      .setDescription(`**${task.title}** has been created.`)
      .setColor(0x22c55e) // emerald-500
      .setTimestamp();
    
    await interaction.editReply({ embeds: [embed] });
  } catch (error) {
    await interaction.editReply({ 
      content: 'Failed to create task. Please try again later.',
      embeds: [] 
    });
  }
}
```

**Component Interactions** (Buttons, Select Menus, Modals):
- **Custom ID format**: `action:entity:id` (e.g., `delete:task:123`, `page:users:2`)
- **Timeout handling**: Buttons expire after 15 minutes (Discord limit), manage custom timeout
- **State preservation**: Store interaction state in Redis with TTL
- **Permission validation**: Re-check permissions on every interaction (not just command invocation)

**Message Components Design**:
```typescript
// Action rows with buttons
const row = new ActionRowBuilder<ButtonBuilder>()
  .addComponents(
    new ButtonBuilder()
      .setCustomId('approve:request:123')
      .setLabel('Approve')
      .setStyle(ButtonStyle.Success)
      .setEmoji({ name: 'check', id: 'custom_id' }), // Custom emoji only, no Unicode emoji in code
    new ButtonBuilder()
      .setCustomId('reject:request:123')
      .setLabel('Reject')
      .setStyle(ButtonStyle.Danger),
    new ButtonBuilder()
      .setCustomId('details:request:123')
      .setLabel('View Details')
      .setStyle(ButtonStyle.Secondary)
  );
```

---

### SECTION B5: DESIGN SYSTEM FOR BOTS

**Embed Design Standards** (Discord/Universal):
```
Color Semantics:
- Success: #22c55e (emerald-500)
- Error: #ef4444 (red-500)
- Warning: #f59e0b (amber-500)
- Info: #3b82f6 (blue-500)
- Neutral: #6b7280 (gray-500)

Embed Structure:
- Title: Max 256 chars, bold, action-oriented
- Description: Max 4096 chars, markdown supported
- Fields: Inline (3 per row) or block, name (256) + value (1024)
- Footer: Timestamp + bot name
- Thumbnail: 80x80px, top-right
- Image: 600x400px max, bottom
- Author: Icon + name, top

NO EMOJI in code: Use custom guild emojis with IDs, or platform-native reactions only. Never hardcode Unicode emoji characters in source.
```

**Message Formatting**:
- Use code blocks for technical data (` ```json `)
- Use blockquotes for user-generated content
- Use spoilers for sensitive info (`||content||`)
- Use timestamps for dates (`<t:1234567890:R>` for relative)
- Paginate lists: 10 items per page, navigation buttons

---

### SECTION B6: CONVERSATION & CONTEXT MANAGEMENT

**Context Window**:
- Store last N messages per channel (configurable, default 20)
- Include bot's own messages in context (self-correction)
- Token budget management for AI integrations
- Summarization: Compress old context when budget exceeded

**Memory Systems**:
- **Short-term**: Redis TTL (5-30 minutes), conversation flow
- **Long-term**: PostgreSQL, user preferences, guild settings
- **Semantic**: Vector DB (Pinecone/pgvector), conversation embedding for recall
- **Entity extraction**: Extract names, dates, topics from conversation for later retrieval

**Multi-Tenancy** (Discord guilds / Slack workspaces):
- Guild-specific prefixes, settings, permissions
- Database schema: `guilds` table with JSON config column
- Isolation: Bot commands scoped to guild, no cross-guild data leakage
- Admin dashboard: Web interface for guild configuration (optional)

---

### SECTION B7: RATE LIMITING & RELIABILITY

**Platform Rate Limits**:
- **Discord**: 5 requests per 5 seconds per route, global 50/10s, interaction response 3-second timeout
- **Slack**: Tiered rate limits per method, webhook burst limits
- **Telegram**: 30 messages/second, 20 messages/minute to same group
- **WhatsApp**: 80 messages/second (Business API), template message limits

**Handling Strategy**:
```typescript
// Exponential backoff with jitter
async function withRetry<T>(
  fn: () => Promise<T>,
  maxRetries: 5,
  baseDelay: 1000
): Promise<T> {
  for (let i = 0; i < maxRetries; i++) {
    try {
      return await fn();
    } catch (error) {
      if (error.status === 429) {
        const delay = baseDelay * Math.pow(2, i) + Math.random() * 1000;
        await sleep(delay);
        continue;
      }
      throw error;
    }
  }
  throw new Error('Max retries exceeded');
}

// Queue-based execution
const messageQueue = new BullMQ('discord-messages', { connection: redis });
messageQueue.add('send-message', payload, {
  attempts: 5,
  backoff: { type: 'exponential', delay: 2000 },
  removeOnComplete: 100,
  removeOnFail: 50
});
```

**Health Checks**:
- Gateway heartbeat monitoring (Discord: heartbeat ACK every ~41s)
- Automatic reconnection with exponential backoff (max 10 minutes)
- Shard health: If shard unresponsive for >5 minutes, restart
- Memory leak detection: Restart process if heap > 1GB

---

### SECTION B8: TESTING & DEPLOYMENT

**Testing**:
- Unit: Command handlers with mocked interactions
- Integration: Full message flow with test bot in isolated guild
- E2E: Real Discord server, automated user account (self-bot forbidden, use official testing)
- Load: Simulate 1000 guilds with message flood

**Deployment**:
- Docker container with health check
- Environment: `DISCORD_TOKEN`, `REDIS_URL`, `DATABASE_URL`
- Graceful shutdown: Close gateway connections, finish processing messages
- Zero-downtime: Blue-green deployment with shared Redis state

---

## PART C: CLI TOOL DEVELOPMENT PROTOCOL

### SECTION C1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **Tool Type**: Utility / Scaffolding / DevOps / Data processing / System admin / AI assistant / Build tool
- **Target Users**: Developers / DevOps / System admins / End users
- **Interaction Model**: Pure command / Interactive wizard / REPL / TUI (Terminal User Interface) / Daemon mode
- **Output Format**: Human-readable tables / JSON / CSV / YAML / TOML / Binary / Streaming
- **Configuration**: CLI flags only / Config file / Environment variables / Interactive setup

**Input / Output Matrix**:

| Input Channel | Format | Validation | Handling |
|--------------|--------|------------|----------|
| CLI arguments | Strings, flags, options | Arg parser validation (clap, commander) | Type coercion, defaults |
| Stdin pipe | Text / binary stream | Buffer size limits, encoding detection | Streaming processing |
| Config files | TOML / YAML / JSON / INI | Schema validation (Zod, JSON Schema) | Merge with defaults |
| Environment vars | KEY=VALUE | Type parsing, prefix filtering | Override config values |
| Interactive prompts | Text, select, confirm, multiselect | Input validation, cancellation | Timeout, default values |
| File system | Any file type | Path validation, permission check | Sandbox to working dir |
| Network | API responses | Timeout, retry, certificate check | Offline fallback, cache |

| Output Channel | Format | Optimization | Fallback |
|---------------|--------|--------------|----------|
| Stdout | Text / ANSI colors | Progressive output, buffering | Plain text (NO_COLOR) |
| Stderr | Error messages, logs | Immediate flush | Redirect to file |
| File write | Any format | Atomic write (temp + rename) | Backup existing file |
| Exit code | 0-255 Unix standard | Semantic codes (0, 1, 2, 64-78) | Always explicit exit |
| Progress bars | ANSI graphics | Terminal width detection | Spinner if width < 20 |
| Tables | Grid with borders | Auto-column width, truncation | CSV if piped |
| JSON / YAML | Structured data | Pretty print (tty) / compact (pipe) | Validate schema before output |
| TUI | Full-screen terminal UI | 60fps rendering, minimal redraw | Fallback to simple output |

---

### SECTION C2: TECH-STACK MANDATE

**Rust (Recommended for Performance)**:
```
CLI Framework: clap v4 (derive macros for commands, subcommands, flags)
TUI: ratatui (Rust TUI framework) + crossterm (cross-platform terminal)
Config: config-rs (TOML/YAML/JSON/ENV layered)
Logging: tracing (structured) + tracing-subscriber (fmt, json)
Colors: owo-colors / ansi_term
Progress: indicatif (progress bars, spinners)
Prompts: inquire (interactive prompts)
HTTP: reqwest (async) + tokio runtime
Serialization: serde + serde_json + toml
Error Handling: anyhow (application) + thiserror (library)
Testing: built-in test + assert_cmd (CLI E2E) + predicates
Build: cargo (release with LTO, strip symbols, static linking)
Distribution: cargo-dist (cross-platform builds, installers, Homebrew)
```

**Node.js / TypeScript (Rapid Development)**:
```
CLI Framework: commander.js or oclif (enterprise-grade, plugins)
TUI: ink (React for terminal) or blessed
Config: cosmiconfig (auto-discovery) + zod validation
Logging: pino (structured, json) or winston
Colors: chalk / picocolors
Progress: ora (spinners) + cli-progress
Prompts: inquirer / enquirer / @inquirer/prompts
HTTP: undici / axios
Testing: vitest (unit) + execa (E2E CLI testing)
Build: tsc + pkg (binary compilation) or nexe
Distribution: npm publish + npx install, Homebrew formula
```

**Go (Systems Tooling)**:
```
CLI Framework: cobra (commands) + viper (config)
TUI: bubbletea (Elm architecture) + lipgloss (styling)
Config: Viper (multi-format, env auto-bind)
Logging: slog (structured, Go 1.21+)
Colors: fatih/color
Progress: cheggaaa/pb or schollz/progressbar/v3
HTTP: net/http standard library
Testing: testing package + testify
Build: go build (static binary by default)
Distribution: GoReleaser (cross-compile, release, Homebrew, Scoop, Chocolatey)
```

**Python (Data/ML Tools)**:
```
CLI Framework: Click or Typer (type hints based)
TUI: Textual (Python TUI framework) or rich
Config: pydantic-settings (env + config file)
Logging: structlog (structured) or standard logging
Colors: rich (console markup)
Progress: rich.progress or tqdm
Prompts: questionary (Inquirer.js port)
HTTP: httpx (async) or requests
Testing: pytest + click.testing.CliRunner
Build: PyInstaller (binary) or pip install
Distribution: PyPI + Homebrew + pipx
```

---

### SECTION C3: SYSTEM ARCHITECTURE

```
┌─────────────────────────────────────────────────────────────┐
│                    CLI INTERFACE LAYER                       │
│  Arg Parser → Command Router → Subcommand Dispatcher         │
│  Help Generator → Shell Completion → Man Page Generator      │
├─────────────────────────────────────────────────────────────┤
│                    INTERACTION LAYER                         │
│  Interactive Prompts → TUI Renderer → Progress Indicators    │
│  Spinner Manager → Table Formatter → JSON/YAML Emitter       │
├─────────────────────────────────────────────────────────────┤
│                    BUSINESS LOGIC LAYER                      │
│  Commands → Services → Use Cases → Domain Logic              │
│  File Processors → API Clients → Background Workers          │
├─────────────────────────────────────────────────────────────┤
│                    CONFIGURATION LAYER                       │
│  Defaults → Config File → Environment → CLI Flags            │
│  (Precedence: CLI > ENV > Config > Defaults)                 │
├─────────────────────────────────────────────────────────────┤
│                    INFRASTRUCTURE LAYER                      │
│  HTTP Client → Cache (disk) → Logger → Error Reporter        │
│  File System → Process Spawner → Plugin Loader               │
└─────────────────────────────────────────────────────────────┘
```

---

### SECTION C4: ARGUMENT PARSING & COMMAND DESIGN

**Command Structure**:
```
myapp [global options] <command> [command options] [arguments]

Global Options:
  -c, --config <path>     Config file path
  -v, --verbose           Increase verbosity (-v, -vv, -vvv)
  -q, --quiet             Suppress non-error output
  --no-color              Disable ANSI colors
  --json                  Output JSON instead of human-readable
  -h, --help              Show help
  -V, --version           Show version

Commands:
  init                    Initialize project / config
  login                   Authenticate with service
  status                  Show current status / context
  list, ls                List resources
  create, new             Create new resource
  get, show               Get resource details
  update, edit            Update resource
  delete, rm              Delete resource
  config                  Manage configuration
  completion              Generate shell completion script
  upgrade                 Check for updates / self-update
```

**Exit Codes (Semantic)**:
```
0   - Success
1   - General error
2   - Misuse of command (wrong args, invalid syntax)
64  - Command line usage error
65  - Data format error
66  - Cannot open input
67  - Addressee unknown
68  - Host name unknown
69  - Service unavailable
70  - Internal software error
77  - Permission denied
78  - Configuration error
130 - Interrupted by user (Ctrl+C)
```

---

### SECTION C5: DESIGN SYSTEM FOR CLI

**Visual Output Standards**:
- **Colors**: Use semantic colors only — Green (success), Red (error), Yellow (warning), Blue (info), Gray (muted)
- **NO EMOJI**: Use ASCII/Unicode symbols — `[OK]`, `[FAIL]`, `[WARN]`, `[INFO]`, `->`, `=>`, `✓` (checkmark U+2713), `✗` (U+2717), `→` (U+2192), `⚡` (U+26A1 only if absolutely necessary, prefer text)
- **Tables**: Box-drawing characters (`┌─┬─┐`), auto-width, truncation with ellipsis, right-align numbers
- **Lists**: Bullet points (`•` or `-`), indentation (2 spaces), grouped with headers
- **JSON/YAML**: Syntax highlighting in TTY, compact in pipes, pretty-print with 2-space indent
- **Progress bars**: `[=====>    ] 45%` or Unicode block characters (`█`, `░`)
- **Spinners**: Rotating ASCII (`|`, `/`, `-`, `\`) or bouncing dot

**Typography in Terminal**:
- Bold for headers and emphasis
- Dim for secondary info, timestamps, metadata
- Italic for file paths, variable names
- Underline for URLs, interactive elements
- Strikethrough for deprecated items

**TUI Design (If Applicable)**:
- **Layout**: Header (title + status) + Main content (scrollable) + Footer (shortcuts + help)
- **Navigation**: Arrow keys, Vim keys (hjkl), Tab/Shift+Tab, Page Up/Down, Home/End
- **Focus indicators**: Bold border, background color change, cursor
- **Mouse support**: Click to focus, scroll, select (optional but recommended)
- **Responsive**: Adapt to terminal size (minimum 80x24, optimal 120x40)

---

### SECTION C6: CONFIGURATION MANAGEMENT

**Config File Locations** (XDG Base Directory compliant):
```
Linux:   ~/.config/myapp/config.toml
macOS:   ~/Library/Application Support/myapp/config.toml
Windows: %APPDATA%\myapp\config.toml
```

**Config Precedence** (Highest to Lowest):
1. CLI flags (`--option value`)
2. Environment variables (`MYAPP_OPTION=value`)
3. Local config (`./.myapp.toml` — project-specific)
4. User config (`~/.config/myapp/config.toml`)
5. System config (`/etc/myapp/config.toml`)
6. Built-in defaults

**Config Schema** (TOML example):
```toml
[core]
editor = "vim"
pager = "less"
theme = "auto"  # auto, light, dark

[api]
url = "https://api.service.com"
timeout = 30
retries = 3

[auth]
token = ""  # Loaded from env MYAPP_TOKEN or keyring

[output]
format = "auto"  # auto, table, json, yaml, csv
no_color = false
quiet = false

[aliases]
st = "status"
co = "checkout"
```

---

### SECTION C7: SHELL INTEGRATION

**Tab Completions**:
- Bash: `_myapp()` function in `/etc/bash_completion.d/` or user dir
- Zsh: `_myapp` compdef in `$fpath`
- Fish: `myapp.fish` in `~/.config/fish/completions/`
- PowerShell: `Register-ArgumentCompleter` script
- Auto-generation: CLI framework generates completion scripts (`myapp completion bash`)

**Man Pages**:
- Section 1: User commands
- Auto-generated from help text or markdown source
- Install to `/usr/local/share/man/man1/` or `~/.local/share/man/man1/`

**Aliases & Shortcuts**:
- Support shell aliases natively (configurable command aliases)
- Shortcut commands: `myapp s` → `myapp status` if unambiguous

---

### SECTION C8: TESTING STRATEGY

**Unit Tests**:
- Pure functions: Input → Output assertions
- Command handlers: Mocked services, verify exit codes
- Config parsing: Valid/invalid config files, precedence testing

**Integration Tests**:
- File system operations: Temp directories, cleanup
- API client: Mock server (msw, wiremock, httptest)
- Config loading: Multiple sources, override behavior

**E2E Tests**:
- Full CLI invocation with real binary
- Assert stdout, stderr, exit code, file system side effects
- Tools: `assert_cmd` (Rust), `execa` (Node), `pytest` + `subprocess` (Python)
- Snapshot testing: Compare output to stored snapshots for regression detection

---

### SECTION C9: DISTRIBUTION

**Package Managers**:
- **npm**: `npm install -g myapp` (Node tools)
- **Homebrew**: `brew install myapp` (macOS/Linux)
- **Scoop**: `scoop install myapp` (Windows)
- **Chocolatey**: `choco install myapp` (Windows)
- **AUR**: `yay -S myapp` (Arch Linux)
- **Cargo**: `cargo install myapp` (Rust)
- **Go**: `go install github.com/user/myapp@latest`
- **PyPI**: `pipx install myapp` (Python)
- **Snap**: `snap install myapp` (Linux universal)
- **Direct**: Binary download from GitHub Releases

**Self-Updater**:
- Check GitHub Releases API or custom endpoint
- Download latest binary for current platform/arch
- Verify checksum / signature
- Atomic replacement (rename old, move new, fallback on error)
- `myapp upgrade` command

---

## PART D: API SERVICE DEVELOPMENT PROTOCOL

### SECTION D1: PROJECT INTELLIGENCE & DISCOVERY

**Aim Extraction**:
- **API Type**: REST / GraphQL / gRPC / tRPC / WebSocket / SSE / Webhook provider
- **Audience**: Internal microservices / Partner integrations / Public developer platform / Mobile app backend
- **Scale**: Startup MVP / Scale-up / Enterprise / Platform (multi-tenant)
- **SLA Targets**: 99.9% uptime, P50 < 50ms, P95 < 200ms, P99 < 500ms
- **Compliance**: SOC-2, GDPR, HIPAA, PCI-DSS, ISO 27001

**Input / Output Matrix**:

| Input Channel | Format | Validation | Handling |
|--------------|--------|------------|----------|
| HTTP requests | JSON / XML / form-data / binary | Content-Type, body parser limits | Route handler, middleware chain |
| GraphQL queries | Query + variables + operation | Schema validation, depth limiting | Resolver execution, dataloader batching |
| gRPC calls | Protobuf binary | Protocol version, message size | Service method handler |
| WebSocket frames | JSON / binary | Schema validation, auth token | Connection manager, room routing |
| Webhooks | POST JSON / form | Signature verification (HMAC), replay check | Idempotent processing, queue |
| File uploads | multipart/form-data | MIME type, magic bytes, size limit | Stream to storage, virus scan |
| Headers | Auth tokens, content negotiation | Format validation (Bearer, API key) | Auth middleware, content negotiation |

| Output Channel | Format | Optimization | Fallback |
|---------------|--------|--------------|----------|
| HTTP responses | JSON / XML / binary | Compression (Brotli/gzip), caching | Error format (RFC 7807) |
| GraphQL responses | JSON with data/errors | Query result caching, persisted queries | Partial response (null fields) |
| gRPC responses | Protobuf binary | HTTP/2 multiplexing, streaming | gRPC-Web fallback |
| SSE streams | text/event-stream | Connection keep-alive, reconnection ID | Long-polling fallback |
| WebSocket messages | JSON / binary | Binary for large payloads | Reconnect with exponential backoff |
| Webhook deliveries | POST JSON | Retry with backoff (max 5), circuit breaker | Dead letter queue |

---

### SECTION D2: TECH-STACK MANDATE

**REST API (Recommended Default)**:
```
Runtime: Node.js 20 LTS or Bun 1.1+
Framework: Fastify (performance) / Express (ecosystem) / Hono (edge)
Language: TypeScript 5.x (strict)
Validation: Zod (runtime) + JSON Schema (OpenAPI)
Auth: OAuth 2.0 + PKCE / JWT (access + refresh) / API Keys (scoped)
Database: PostgreSQL 16 + Drizzle ORM / Prisma
Cache: Redis 7 (caching, sessions, rate limits)
Queue: BullMQ / RabbitMQ / Apache Kafka
Search: Meilisearch / Algolia / Elasticsearch
Documentation: OpenAPI 3.1 + Scalar / Swagger UI / Redoc
Testing: Vitest (unit) + Supertest (integration) + k6 (load)
Monitoring: Prometheus + Grafana + Jaeger (tracing) + Sentry (errors)
```

**GraphQL API**:
```
Server: Apollo Server / Yoga / Pothos (code-first schema)
Client: Relay / Apollo Client / urql
Features: DataLoader (N+1 prevention), persisted queries, query cost analysis
Subscriptions: WebSocket (graphql-ws) or SSE (graphql-sse)

Schema Design:
- Queries: Read operations, paginated lists
- Mutations: Write operations, input objects, payload objects
- Subscriptions: Real-time events, filtered by user
- Types: Interface + Union for polymorphism, Enums for fixed values
```

**gRPC API**:
```
Protocol Buffers: proto3 syntax
Server: bufbuild/connect-go (Go) / tonic (Rust) / grpc-node (Node)
Features: Bi-directional streaming, flow control, metadata
Gateway: gRPC-Web (for browser) or gRPC-Gateway (REST fallback)
Tools: buf (lint, breaking change detection, code generation)
```

---

### SECTION D3: SYSTEM ARCHITECTURE

```
┌─────────────────────────────────────────────────────────────┐
│                    EDGE / CDN LAYER                          │
│  Cloudflare / AWS CloudFront → DDoS protection, caching     │
│  WAF rules, bot management, geographic routing              │
├─────────────────────────────────────────────────────────────┤
│                    API GATEWAY LAYER                         │
│  Kong / AWS API Gateway / Traefik / Caddy                   │
│  Rate limiting, auth validation, request routing, SSL        │
├─────────────────────────────────────────────────────────────┤
│                    APPLICATION LAYER                         │
│  Load Balancer → App Instances (horizontal scaling)          │
│  Middleware: Logging, metrics, tracing, CORS, compression    │
│  Route handlers → Controllers → Services → Repositories      │
├─────────────────────────────────────────────────────────────┤
│                    BUSINESS LOGIC LAYER                      │
│  Use cases → Domain services → Event handlers                │
│  Background workers → Scheduled jobs → External integrations │
├─────────────────────────────────────────────────────────────┤
│                    DATA LAYER                                │
│  PostgreSQL (primary) → Read replicas → Connection pool     │
│  Redis (cache + sessions + rate limits + pub/sub)           │
│  S3/R2 (file storage) → CDN delivery                        │
│  Meilisearch (full-text search) → Elasticsearch (analytics)  │
│  Kafka / RabbitMQ (event streaming, async processing)       │
└─────────────────────────────────────────────────────────────┘
```

---

### SECTION D4: API DESIGN STANDARDS

**RESTful Design**:
```
Resource Naming:
GET    /api/v1/users              → List users (paginated)
GET    /api/v1/users/:id          → Get user by ID
POST   /api/v1/users              → Create user
PATCH  /api/v1/users/:id          → Partial update
PUT    /api/v1/users/:id          → Full replacement
DELETE /api/v1/users/:id          → Delete user
GET    /api/v1/users/:id/orders   → Sub-resource collection
POST   /api/v1/users/:id/orders   → Create sub-resource

HTTP Status Codes:
200 - OK (GET, PATCH, PUT success)
201 - Created (POST success)
204 - No Content (DELETE success)
400 - Bad Request (validation error)
401 - Unauthorized (missing/invalid auth)
403 - Forbidden (valid auth, insufficient permissions)
404 - Not Found
409 - Conflict (duplicate, state mismatch)
422 - Unprocessable Entity (semantic errors)
429 - Too Many Requests (rate limit)
500 - Internal Server Error
503 - Service Unavailable (maintenance, overload)
```

**Error Format (RFC 7807 Problem Details)**:
```json
{
  "type": "https://api.yourservice.com/errors/validation-failed",
  "title": "Validation Failed",
  "status": 400,
  "detail": "The request body contains invalid fields.",
  "instance": "/api/v1/users",
  "errors": [
    {
      "field": "email",
      "message": "Must be a valid email address",
      "code": "invalid_email"
    }
  ],
  "timestamp": "2026-08-26T10:30:00Z",
  "requestId": "req_abc123xyz"
}
```

**Pagination**:
```json
// Cursor-based (recommended for high-performance lists)
{
  "data": [...],
  "pagination": {
    "nextCursor": "eyJpZCI6MTAwfQ==",
    "hasMore": true,
    "totalCount": 5000
  }
}

// Offset-based (acceptable for admin/administrative UIs)
{
  "data": [...],
  "pagination": {
    "page": 2,
    "perPage": 20,
    "totalPages": 250,
    "totalCount": 5000
  }
}
```

**Versioning Strategy**:
- URL path: `/api/v1/`, `/api/v2/` (recommended)
- Header: `Accept: application/vnd.api.v2+json` (alternative)
- Sunset policy: 12-month deprecation window, migration guide, email notifications

---

### SECTION D5: AUTHENTICATION & AUTHORIZATION

**Authentication Methods**:
1. **OAuth 2.0 + OpenID Connect**: Authorization code flow with PKCE (SPA/mobile), client credentials (M2M)
2. **JWT Access Tokens**: Short-lived (15 min), signed (RS256), contain claims (sub, iss, aud, scope, exp)
3. **Refresh Tokens**: Long-lived (7-30 days), rotation on use, stored securely (httpOnly cookie or secure storage)
4. **API Keys**: For server-to-server, scoped, prefix identifiable (`live_`, `test_`), hashed in DB, last used tracking
5. **mTLS**: Mutual TLS for microservice communication, no additional tokens needed

**Authorization Patterns**:
- **RBAC**: Role-Based (admin, editor, viewer) — simple, hierarchical
- **ABAC**: Attribute-Based (user department = resource department) — flexible, complex
- **Policy Engine**: OPA (Open Policy Agent) or Casbin for fine-grained decisions
- **Scope-based**: OAuth scopes (`read:users`, `write:orders`) checked per endpoint

**Implementation**:
```typescript
// Middleware chain
app.use(authenticate);     // Verify JWT / API key
app.use(rateLimit);        // Token bucket per user/IP
app.use(authorize('users:read')); // Check scope/permission
app.use(validateRequest);  // Zod schema validation
app.use(controller);       // Business logic
```

---

### SECTION D6: RATE LIMITING & THROTTLING

**Strategies**:
- **Token Bucket**: Burst capacity + refill rate (best for APIs)
- **Sliding Window**: Precise count per time window (best for strict limits)
- **Fixed Window**: Simple, but allows burst at window boundary
- **Leaky Bucket**: Smooth output rate (best for webhooks)

**Headers** (Standard):
```
X-RateLimit-Limit: 1000
X-RateLimit-Remaining: 999
X-RateLimit-Reset: 1693045800
X-RateLimit-Retry-After: 60
Retry-After: 60
```

**Tiered Limits**:
| Tier | Requests/min | Requests/day | Burst |
|------|-------------|--------------|-------|
| Free | 60 | 1,000 | 10 |
| Pro | 600 | 50,000 | 50 |
| Enterprise | 6,000 | Unlimited | 200 |
| Internal | Unlimited | Unlimited | Unlimited |

---

### SECTION D7: CACHING STRATEGY

**Layers**:
1. **Browser/CDN**: `Cache-Control: public, max-age=3600, stale-while-revalidate=86400`
2. **Application**: Redis cache for hot data (user profiles, config, reference data)
3. **Database**: Query result cache, materialized views for reports
4. **ORM**: Second-level cache (Prisma Accelerate, Drizzle cache)

**Cache Invalidation**:
- **Time-based**: TTL (Time To Live) — simple, stale data risk
- **Event-based**: Cache key deletion on write — accurate, complex
- **Write-through**: Update cache on write — consistent, slower writes
- **Write-behind**: Async cache update — fast writes, eventual consistency

**Cache Keys**:
- `user:{id}:profile`
- `users:list:{cursor}:{limit}:{filters_hash}`
- `config:app:features`

---

### SECTION D8: WEBHOOKS & EVENTS

**Webhook Delivery**:
- **Signature**: HMAC-SHA256 of payload + timestamp, verify on receiver
- **Retry**: Exponential backoff (1s, 2s, 4s, 8s, 16s), max 5 attempts
- **Idempotency**: `Idempotency-Key` header, receiver stores processed keys for 24h
- **Timeout**: 30 seconds per delivery attempt
- **Circuit breaker**: Open after 5 consecutive failures, half-open after 60s
- **Batching**: Optional batch delivery for high-volume events

**Event Schema**:
```json
{
  "id": "evt_abc123",
  "type": "invoice.paid",
  "api_version": "2024-08-01",
  "created_at": "2026-08-26T10:30:00Z",
  "data": {
    "object": { /* event payload */ },
    "previous_attributes": { /* changed fields */ }
  }
}
```

---

### SECTION D9: TESTING & OBSERVABILITY

**Testing**:
- **Unit**: Services, utilities, validators (mocked dependencies)
- **Integration**: API endpoints with test database, verify status codes, response shapes, DB state
- **Contract**: Pact or OpenAPI validation (consumer-driven contracts)
- **Load**: k6 or Artillery — 1000 RPS sustained, burst to 5000 RPS
- **Chaos**: Random failure injection (latency spikes, 500 errors, DB restarts)
- **Security**: OWASP ZAP, Burp Suite, fuzzing inputs

**Observability**:
- **Metrics**: RED method (Rate, Errors, Duration) + USE method (Utilization, Saturation, Errors)
- **Logging**: Structured JSON, correlation IDs, request tracing, no PII
- **Tracing**: OpenTelemetry + Jaeger/Zipkin, distributed trace across services
- **Alerting**: PagerDuty/Opsgenie — latency P95 > 500ms, error rate > 0.1%, 5xx rate > 0.01%

---

### SECTION D10: DOCUMENTATION & SDKs

**API Documentation**:
- OpenAPI 3.1 spec auto-generated from code
- Interactive docs: Scalar, Swagger UI, Redoc
- Examples: curl, JavaScript, Python, Go for every endpoint
- Changelog: Breaking vs non-breaking changes, migration guides

**SDK Generation**:
- Auto-generate from OpenAPI: `openapi-generator` or `fern`
- Languages: TypeScript, Python, Go, Ruby, Java, C#
- Features: Retry logic, pagination helpers, auth handling, type safety

---

## PART E: UNIFIED FINAL CHECKLIST

### For Browser Extensions:
- [ ] Manifest V3 compliant, CSP strict, no inline scripts
- [ ] Content scripts use Shadow DOM, no CSS leakage
- [ ] Background service worker handles events, no persistent state assumptions
- [ ] Cross-browser: Chrome + Firefox tested, Safari if required
- [ ] Store listing complete: icons, screenshots, description, privacy policy
- [ ] Permissions minimal, optional where possible
- [ ] No emoji in UI — vector icons only
- [ ] Minimum 10,000 lines for complex extension platforms

### For Bots:
- [ ] Command handlers typed and validated (Zod)
- [ ] Rate limiting respects platform limits with backoff
- [ ] Ephemeral replies for sensitive operations
- [ ] Context/conversation management implemented
- [ ] Multi-tenancy isolation (guild/workspace scoped)
- [ ] Background jobs queued (BullMQ / Redis)
- [ ] Graceful shutdown, reconnection logic robust
- [ ] No hardcoded Unicode emoji — custom emoji IDs or text only

### For CLI Tools:
- [ ] Semantic exit codes, comprehensive help text
- [ ] Shell completions generated for bash/zsh/fish/powershell
- [ ] Configuration: XDG compliant, layered precedence
- [ ] TUI mode optional but polished if present
- [ ] Progress indicators for long operations
- [ ] JSON/YAML/CSV output modes for scripting
- [ ] NO_COLOR support, respects `CI` environment variable
- [ ] Self-updater or clear upgrade path
- [ ] No emoji in terminal output — ASCII/Unicode symbols only
- [ ] Minimum 10,000 lines for comprehensive tool suites

### For API Services:
- [ ] OpenAPI 3.1 spec generated and accurate
- [ ] RFC 7807 error format consistently applied
- [ ] Rate limiting with standard headers
- [ ] Authentication: OAuth2 / JWT / API Keys with rotation
- [ ] Authorization: RBAC/ABAC with scope checking
- [ ] Pagination: Cursor-based for lists, offset for admin
- [ ] Caching strategy documented and implemented
- [ ] Webhook delivery: signed, retried, idempotent
- [ ] Database: Migrations versioned, reversible, tested
- [ ] Observability: Metrics, logs, traces, alerting
- [ ] Load tested: 1000+ concurrent users, graceful degradation
- [ ] Security: No secrets in code, dependency scanning, SAST/DAST
- [ ] SDKs generated or hand-written for top 3 languages
- [ ] No emoji in API responses or documentation — icons or text only
- [ ] Minimum 10,000 lines for production-grade services

---

**REMEMBER FOR ALL FOUR CATEGORIES**:
- NO EMOJI CHARACTERS anywhere in codebase, documentation, or UI
- MINIMUM 10,000 lines of production-grade code for full projects
- Every section above must be visibly satisfied in output
- When in doubt, over-engineer rather than under-deliver
- Include comprehensive tests, CI/CD, Docker, and documentation

---

*End of GOVINDA CLI Browser Extensions, Bots, CLI Tools & API Services Protocol v5.0*