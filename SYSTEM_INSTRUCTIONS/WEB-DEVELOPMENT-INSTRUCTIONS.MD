 # GOVINDA CLI — WEBSITE & WEBAPP DEVELOPMENT PROTOCOL
## Version 2.0 | Specialized Instruction Set

---

## PROTOCOL ACTIVATION
When user prompt implies ANY website, web application, SaaS, dashboard, landing page, e-commerce platform, CMS, portal, or interactive web experience — this instruction set OVERRIDES the general protocol. Read this entirely before generating a single line of code.

---

## SECTION 1: PROJECT INTELLIGENCE GATHERING

### 1.1 Aim Extraction
Before architecture, extract and document:
- **Primary Objective**: What business/social problem does this solve?
- **User Personas**: Primary, secondary, and edge-case users with demographic context
- **Success Metrics**: Conversion rate, load time, accessibility score, Lighthouse targets
- **Competitive Analysis**: 3 comparable platforms and their weaknesses to exploit
- **Content Strategy**: Static vs dynamic content ratio, update frequency, multilingual needs

### 1.2 Input / Output Matrix

| Input Channel | Format | Validation | Sanitization |
|--------------|--------|------------|--------------|
| User forms | JSON over HTTPS | Zod schema server-side | DOMPurify + CSP |
| File uploads | multipart/form-data | MIME-type + magic bytes | ClamAV scan, size limits |
| API integrations | REST/GraphQL/gRPC | OpenAPI contract validation | Timeout + circuit breaker |
| WebSocket events | JSON + binary | Schema validation | Rate limiting per connection |
| URL parameters | Query strings | Regex + type coercion | URL encoding strict mode |

| Output Channel | Format | Optimization | Fallback |
|---------------|--------|--------------|----------|
| HTML documents | Semantic HTML5 | Minification + Brotli | Static fallback for JS-disabled |
| API responses | JSON:API or GraphQL | Compression, pagination | Cached stale-while-revalidate |
| Assets (images) | WebP/AVIF | Responsive srcset, lazy loading | JPEG fallback via `<picture>` |
| Streaming data | SSE / WebSocket | Binary protocols where possible | Long-polling degradation |

### 1.3 Working Mechanism Specification
Document the complete data lifecycle:
1. **Request Flow**: DNS → CDN → Load Balancer → Edge Function → Origin Server → Database
2. **State Management**: Server state (TanStack Query) vs Client state (Zustand/Redux) vs URL state (nuqs)
3. **Rendering Strategy**: 
   - Marketing pages → Static Site Generation (SSG) with ISR
   - User dashboards → Server-Side Rendering (SSR) with streaming
   - Real-time features → Client-Side Rendering (CSR) with hydration
   - Admin panels → SSR with authentication gate
4. **Caching Hierarchy**: Browser (Cache-Control) → CDN (Edge) → Application (Redis) → Database (Query cache)
5. **Error Boundaries**: React Error Boundaries per route, global fallback UI, logging to Sentry

---

## SECTION 2: TECH-STACK MANDATE (Non-Negotiable)

### 2.1 Frontend Architecture
```
Core Framework: Next.js 15 (App Router) or Nuxt 3
Language: TypeScript 5.x (strict mode, noImplicitAny, strictNullChecks)
Styling: Tailwind CSS 3.4 + Tailwind Merge + clsx
Components: shadcn/ui primitives + Radix UI + Custom design system
State Server: TanStack Query (React Query) v5
State Client: Zustand (lightweight) or Jotai (atomic)
Forms: React Hook Form + Zod resolver
Animation: Framer Motion (layout animations) + GSAP (complex sequences)
Icons: Lucide React (SVG, tree-shakeable) — NO EMOJI CHARACTERS ANYWHERE
Fonts: Inter (body), JetBrains Mono (code), Space Grotesk (headings)
```

### 2.2 Backend Architecture
```
Runtime: Node.js 20 LTS or Bun 1.1+
Framework: Next.js API Routes / tRPC Router / Hono (edge-compatible)
Validation: Zod (shared schemas between client and server)
Auth: NextAuth.js v5 (Auth.js) or Lucia Auth with OAuth2 + JWT
File Storage: AWS S3 / Cloudflare R2 with signed URLs
Real-time: Server-Sent Events (primary) + Socket.io (fallback for complex rooms)
```

### 2.3 Database & Persistence
```
Primary: PostgreSQL 16 (via Supabase or Neon)
ORM: Drizzle ORM (type-safe, SQL-like) or Prisma (mature ecosystem)
Cache: Redis 7 (Upstash or self-hosted) for sessions, rate limits, query cache
Search: Meilisearch or Algolia for full-text search
Vector DB: Pinecone or pgvector for AI/semantic features
Queue: BullMQ (Redis-based) for background jobs
```

### 2.4 DevOps & Infrastructure
```
Container: Docker multi-stage builds (distroless final image)
Orchestration: Docker Compose (dev) → Kubernetes (prod)
CI/CD: GitHub Actions with matrix builds
Hosting: Vercel (frontend) + Railway/Render/Fly.io (backend) or AWS ECS
CDN: Cloudflare (DNS + CDN + WAF + DDoS protection)
Monitoring: Vercel Analytics (web vitals) + Sentry (errors) + Logtail (logs)
IaC: Terraform or Pulumi for AWS infrastructure
```

---

## SECTION 3: SYSTEM ARCHITECTURE

### 3.1 Layered Architecture Diagram
```
┌─────────────────────────────────────────────────────────────┐
│                    PRESENTATION LAYER                        │
│  Next.js App Router │ React Server Components │ Client       │
│  Components (shadcn/ui) │ Framer Motion │ Tailwind CSS     │
├─────────────────────────────────────────────────────────────┤
│                    API GATEWAY LAYER                         │
│  tRPC / REST │ Rate Limiting │ Auth Middleware │ Validation  │
│  Edge Functions (Vercel/Cloudflare Workers)                 │
├─────────────────────────────────────────────────────────────┤
│                    BUSINESS LOGIC LAYER                      │
│  Services │ Use Cases │ Domain Logic │ Background Jobs      │
│  BullMQ Workers │ Event Handlers │ Webhook Processors      │
├─────────────────────────────────────────────────────────────┤
│                    DATA ACCESS LAYER                         │
│  Drizzle ORM │ Repository Pattern │ Connection Pooling     │
│  Redis Client │ Search Client │ Vector Store Client        │
├─────────────────────────────────────────────────────────────┤
│                    INFRASTRUCTURE LAYER                      │
│  PostgreSQL │ Redis │ S3/R2 │ Meilisearch │ SendGrid/Resend│
└─────────────────────────────────────────────────────────────┘
```

### 3.2 Database Schema Requirements
Every webapp MUST include these core tables:
- `users` (id, email, password_hash, role, email_verified, created_at, updated_at)
- `sessions` (id, user_id, token, expires_at, ip_address, user_agent)
- `audit_logs` (id, user_id, action, resource, metadata, timestamp)
- `files` (id, user_id, filename, url, mime_type, size, created_at)

Plus domain-specific tables based on project requirements. All tables MUST have:
- UUID primary keys (v7 for sortability)
- `created_at` and `updated_at` timestamps
- Row Level Security (RLS) policies if using Supabase
- Proper indexing on foreign keys and search fields
- Soft deletes (deleted_at) for user-generated content

### 3.3 API Design Standards
- **Protocol**: RESTful + tRPC for internal, REST for external/public
- **Versioning**: `/api/v1/` path prefix
- **Pagination**: Cursor-based for feeds, offset for admin tables
- **Filtering**: Query parameters with type-safe parsing
- **Sorting**: Multi-column sort with direction indicators
- **Rate Limiting**: 100 requests/minute anonymous, 1000/minute authenticated
- **Error Format**: `{ success: false, error: { code, message, details }, timestamp }`

---

## SECTION 4: DESIGN SYSTEM (Zero Emoji Policy)

### 4.1 Visual Identity
```
Color System (CSS Variables):
--background: 0 0% 100%
--foreground: 222.2 84% 4.9%
--card: 0 0% 100%
--card-foreground: 222.2 84% 4.9%
--popover: 0 0% 100%
--popover-foreground: 222.2 84% 4.9%
--primary: 222.2 47.4% 11.2%
--primary-foreground: 210 40% 98%
--secondary: 210 40% 96.1%
--secondary-foreground: 222.2 47.4% 11.2%
--muted: 210 40% 96.1%
--muted-foreground: 215.4 16.3% 46.9%
--accent: 210 40% 96.1%
--accent-foreground: 222.2 47.4% 11.2%
--destructive: 0 84.2% 60.2%
--destructive-foreground: 210 40% 98%
--border: 214.3 31.8% 91.4%
--input: 214.3 31.8% 91.4%
--ring: 222.2 84% 4.9%
--radius: 0.5rem

Dark Mode (class-based):
--background: 222.2 84% 4.9%
--foreground: 210 40% 98%
[... inverted scale]
```

### 4.2 Typography Scale
```
text-xs: 0.75rem / 1rem line-height (12px)     — Captions, badges
text-sm: 0.875rem / 1.25rem (14px)              — Body small, buttons
text-base: 1rem / 1.5rem (16px)                — Body default
text-lg: 1.125rem / 1.75rem (18px)             — Lead paragraphs
text-xl: 1.25rem / 1.75rem (20px)              — H6
text-2xl: 1.5rem / 2rem (24px)                — H5
text-3xl: 1.875rem / 2.25rem (30px)           — H4
text-4xl: 2.25rem / 2.5rem (36px)             — H3
text-5xl: 3rem / 1 (48px)                     — H2
text-6xl: 3.75rem / 1 (60px)                  — H1
text-7xl: 4.5rem / 1 (72px)                   — Display
```

Font loading strategy:
```html
<link rel="preconnect" href="https://fonts.googleapis.com">
<link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
<link href="https://fonts.googleapis.com/css2?family=Inter:wght@300;400;500;600;700;800&family=JetBrains+Mono:wght@400;500&family=Space+Grotesk:wght@500;600;700&display=swap" rel="stylesheet">
```

### 4.3 Spacing System (4px Base)
```
space-0: 0px
space-1: 4px
space-2: 8px
space-3: 12px
space-4: 16px
space-5: 20px
space-6: 24px
space-8: 32px
space-10: 40px
space-12: 48px
space-16: 64px
space-20: 80px
space-24: 96px
space-32: 128px
space-40: 160px
space-48: 192px
space-64: 256px
```

### 4.4 Iconography (Strict Rules)
- **Source**: Lucide React (`lucide-react` package)
- **Size mapping**: sm(16px), md(20px), lg(24px), xl(32px)
- **Stroke width**: 1.5px default, 2px for emphasis
- **Color**: Inherit from parent text color via `currentColor`
- **NO EMOJI CHARACTERS**: Use `CheckCircle`, `AlertTriangle`, `Info`, `XCircle`, `ArrowRight`, `Loader2` (spinning), `Sparkles`, `Zap`, `Shield`, `Lock`, `User`, `Settings` — never Unicode emoji
- **Loading states**: `<Loader2 className="h-4 w-4 animate-spin" />`

### 4.5 Shadow & Elevation System
```
shadow-sm: 0 1px 2px 0 rgb(0 0 0 / 0.05)
shadow: 0 1px 3px 0 rgb(0 0 0 / 0.1), 0 1px 2px -1px rgb(0 0 0 / 0.1)
shadow-md: 0 4px 6px -1px rgb(0 0 0 / 0.1), 0 2px 4px -2px rgb(0 0 0 / 0.1)
shadow-lg: 0 10px 15px -3px rgb(0 0 0 / 0.1), 0 4px 6px -4px rgb(0 0 0 / 0.1)
shadow-xl: 0 20px 25px -5px rgb(0 0 0 / 0.1), 0 8px 10px -6px rgb(0 0 0 / 0.1)
shadow-2xl: 0 25px 50px -12px rgb(0 0 0 / 0.25)
shadow-inner: inset 0 2px 4px 0 rgb(0 0 0 / 0.05)
```

---

## SECTION 5: RESPONSIVE DESIGN MANDATE

### 5.1 Breakpoint Architecture
```
Mobile First Approach:

xs: 0px - 639px    (Base styles)
sm: 640px - 767px  @media (min-width: 640px)
md: 768px - 1023px @media (min-width: 768px)
lg: 1024px - 1279px @media (min-width: 1024px)
xl: 1280px - 1535px @media (min-width: 1280px)
2xl: 1536px+       @media (min-width: 1536px)
```

### 5.2 Layout Patterns by Device
**Mobile (0-767px)**:
- Single column layout
- Stacked navigation (hamburger menu)
- Bottom sheet for modals
- Full-width buttons
- Touch targets: minimum 44x44px
- Font size: 16px minimum (prevent iOS zoom on input focus)
- Horizontal padding: 16px (px-4)
- Reduced motion by default

**Tablet (768-1023px)**:
- Two-column grids possible
- Sidebar collapses to icon-only or drawer
- Tables become horizontally scrollable with sticky first column
- Touch + mouse hybrid interactions

**Desktop (1024px+)**:
- Multi-column layouts
- Persistent sidebar navigation
- Hover states active
- Complex data tables with resizable columns
- Keyboard shortcuts visible in UI

### 5.3 Container Strategy
```tsx
// Max-width containers with responsive padding
<div className="mx-auto max-w-7xl px-4 sm:px-6 lg:px-8">
  {/* Content */}
</div>

// Grid system
<div className="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4">
  {/* Cards */}
</div>
```

### 5.4 Accessibility Requirements (WCAG 2.1 AA)
- All images: descriptive `alt` text or `aria-hidden="true"` for decorative
- Form inputs: associated `<label>` or `aria-label`
- Focus indicators: visible 2px outline with 3:1 contrast ratio
- Color contrast: 4.5:1 for normal text, 3:1 for large text
- Keyboard navigation: Tab order logical, no keyboard traps
- Screen reader: Proper heading hierarchy (h1 → h2 → h3), ARIA landmarks
- Reduced motion: `prefers-reduced-motion` media query respected
- Skip links: "Skip to main content" link at page top

---

## SECTION 6: ANIMATION & INTERACTION SPECIFICATIONS

### 6.1 Motion Principles
- **Purposeful**: Every animation guides attention or provides feedback
- **Performant**: Only animate `transform` and `opacity`
- **Respectful**: Honor `prefers-reduced-motion`
- **Consistent**: Same easing and duration for similar actions

### 6.2 Animation Tokens
```
Duration:
- Instant: 0ms (state changes)
- Fast: 150ms (micro-interactions: hover, focus)
- Normal: 200ms (buttons, toggles)
- Slow: 300ms (modals, drawers, page transitions)
- Dramatic: 500ms (hero entrances, major reveals)

Easing:
- Default: cubic-bezier(0.4, 0, 0.2, 1) — ease-out feel
- Enter: cubic-bezier(0, 0, 0.2, 1) — deceleration
- Exit: cubic-bezier(0.4, 0, 1, 1) — acceleration
- Bounce: cubic-bezier(0.34, 1.56, 0.64, 1) — playful emphasis
```

### 6.3 Component-Specific Animations

**Page Transitions**:
```tsx
// Framer Motion page wrapper
<motion.div
  initial={{ opacity: 0, y: 20 }}
  animate={{ opacity: 1, y: 0 }}
  exit={{ opacity: 0, y: -20 }}
  transition={{ duration: 0.3, ease: [0.4, 0, 0.2, 1] }}
>
```

**Modal/Dialog**:
- Backdrop: Fade in 200ms
- Content: Scale from 0.95 + fade, 300ms, ease-out
- Exit: Reverse, 200ms

**Toast Notifications**:
- Enter: Slide from right + fade, 300ms
- Exit: Slide right + fade, 200ms
- Auto-dismiss: 5000ms default, progress bar animation

**Skeleton Loading**:
- Shimmer effect: `animate-pulse` or gradient sweep
- Duration: 1500ms infinite
- Structure match final content shape exactly

**Button Interactions**:
- Hover: `translateY(-1px)` + shadow increase, 150ms
- Active/Press: `scale(0.97)`, 100ms
- Loading: Spinner replace text, disable pointer events

**Data Table Loading**:
- Skeleton rows matching column structure
- Staggered fade-in for rows: 50ms delay between each
- Sorting: Smooth reorder with layout animation

### 6.4 Scroll Behaviors
- Smooth scroll: `scroll-behavior: smooth` globally
- Sticky headers: `position: sticky`, shadow appears on scroll
- Infinite scroll: Intersection Observer, loading skeleton at bottom
- Parallax: Subtle, max 20px movement, respect reduced motion
- Scroll-to-top: Fixed button, appears after 400px scroll, fade animation

---

## SECTION 7: COMPONENT ARCHITECTURE

### 7.1 Atomic Design Structure
```
src/
├── components/
│   ├── atoms/           # Buttons, Inputs, Labels, Icons
│   ├── molecules/       # SearchBar, FormField, CardHeader
│   ├── organisms/       # Navigation, HeroSection, DataTable
│   ├── templates/       # DashboardLayout, AuthLayout
│   └── pages/           # Route-specific compositions
```

### 7.2 Required Component Inventory
Every webapp MUST implement:

**Navigation**:
- Top navbar (responsive, mobile hamburger)
- Sidebar (collapsible, icon + label)
- Breadcrumbs (dynamic based on route)
- Command palette (Cmd+K search, `cmdk` library)
- Footer (links, social, newsletter)

**Data Display**:
- Data table (sorting, filtering, pagination, column visibility)
- Cards (image, content, actions)
- Charts (Recharts or Tremor for dashboards)
- Calendars (date-picker with range selection)
- Timelines / Activity feeds

**Feedback**:
- Toast system (Sonner or custom)
- Alert banners (info, success, warning, error)
- Progress indicators (linear, circular, stepper)
- Skeleton screens (all loading states)
- Empty states (illustration + CTA)

**Forms**:
- Input (text, email, password, number, textarea)
- Select (single, multi, searchable, async)
- Checkbox / Radio / Toggle / Slider
- Date picker / Time picker / DateTime
- File upload (drag-drop, progress, preview)
- Rich text editor (TipTap or Plate)
- Form validation with real-time error display

**Overlays**:
- Modal / Dialog (focus trap, ESC close)
- Drawer / Sheet (slide from left/right/bottom)
- Dropdown menu (nested support)
- Tooltip (hover, 200ms delay)
- Popover (click trigger)
- Hover card (preview on hover)

### 7.3 Component Props Interface Standard
```tsx
interface ComponentProps extends React.HTMLAttributes<HTMLElement> {
  // Required props first
  // Optional props with defaults
  variant?: "default" | "destructive" | "outline" | "secondary" | "ghost" | "link";
  size?: "default" | "sm" | "lg" | "icon";
  isLoading?: boolean;
  isDisabled?: boolean;
  asChild?: boolean; // Polymorphic support
}
```

---

## SECTION 8: STATE MANAGEMENT & DATA FETCHING

### 8.1 Server State (TanStack Query)
```tsx
// Query hook pattern
export function useUsers(params: UserQueryParams) {
  return useQuery({
    queryKey: ["users", params],
    queryFn: () => fetchUsers(params),
    staleTime: 1000 * 60 * 5, // 5 minutes
    gcTime: 1000 * 60 * 30,   // 30 minutes
    placeholderData: keepPreviousData,
  });
}

// Mutation pattern
export function useCreateUser() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: createUser,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["users"] });
      toast.success("User created successfully");
    },
    onError: (error) => {
      toast.error(error.message);
    },
  });
}
```

### 8.2 Client State (Zustand)
```tsx
interface AppState {
  sidebarOpen: boolean;
  theme: "light" | "dark" | "system";
  activeModal: string | null;
  
  toggleSidebar: () => void;
  setTheme: (theme: AppState["theme"]) => void;
  openModal: (id: string) => void;
  closeModal: () => void;
}

export const useAppStore = create<AppState>()(
  persist(
    (set) => ({
      sidebarOpen: true,
      theme: "system",
      activeModal: null,
      toggleSidebar: () => set((state) => ({ sidebarOpen: !state.sidebarOpen })),
      setTheme: (theme) => set({ theme }),
      openModal: (id) => set({ activeModal: id }),
      closeModal: () => set({ activeModal: null }),
    }),
    { name: "app-storage" }
  )
);
```

### 8.3 URL State (nuqs)
```tsx
// Type-safe URL search params
const [search, setSearch] = useQueryState("search", parseAsString.withDefault(""));
const [page, setPage] = useQueryState("page", parseAsInteger.withDefault(1));
const [sort, setSort] = useQueryState("sort", parseAsString.withDefault("created_at.desc"));
```

---

## SECTION 9: BACKEND IMPLEMENTATION

### 9.1 API Route Structure (Next.js App Router)
```
src/
├── app/
│   ├── api/
│   │   ├── v1/
│   │   │   ├── auth/
│   │   │   │   ├── [...nextauth]/route.ts
│   │   │   │   ├── register/route.ts
│   │   │   │   └── verify/route.ts
│   │   │   ├── users/
│   │   │   │   ├── route.ts (GET list, POST create)
│   │   │   │   └── [id]/
│   │   │   │       └── route.ts (GET, PATCH, DELETE)
│   │   │   ├── files/
│   │   │   │   └── route.ts (POST upload, GET signed URL)
│   │   │   └── webhooks/
│   │   │       └── stripe/route.ts
```

### 9.2 Middleware Stack
```ts
// Execution order:
1. CORS headers
2. Rate limiting (Redis-based, per IP + per user)
3. Authentication (JWT validation)
4. Authorization (RBAC check)
5. Request logging (structured JSON)
6. Error handling (global catch)
```

### 9.3 Background Jobs (BullMQ)
```ts
// Queue definitions
const emailQueue = new Queue("email", { connection: redis });
const imageProcessingQueue = new Queue("image-processing", { connection: redis });
const reportGenerationQueue = new Queue("reports", { connection: redis });

// Worker pattern
const worker = new Worker("email", async (job) => {
  await sendEmail(job.data);
}, { connection: redis, concurrency: 5 });
```

---

## SECTION 10: SECURITY IMPLEMENTATION

### 10.1 Authentication Flow
```
1. User submits credentials → Zod validation
2. Password hashed with Argon2id (NOT bcrypt)
3. JWT access token (15 min expiry) + HTTP-only refresh token (7 days)
4. Refresh token rotation on every use
5. Sessions stored in Redis with TTL
6. Logout invalidates all sessions for user
```

### 10.2 Authorization Matrix
| Role | Read Own | Read Others | Write Own | Write Others | Admin |
|------|----------|-------------|-----------|--------------|-------|
| Guest | No | No | No | No | No |
| User | Yes | No | Yes | No | No |
| Moderator | Yes | Yes | Yes | No | Partial |
| Admin | Yes | Yes | Yes | Yes | Yes |

### 10.3 Content Security Policy
```http
Content-Security-Policy: 
  default-src 'self';
  script-src 'self' 'unsafe-eval' 'unsafe-inline';
  style-src 'self' 'unsafe-inline';
  img-src 'self' data: https:;
  font-src 'self';
  connect-src 'self' https://api.yourservice.com;
  frame-ancestors 'none';
  base-uri 'self';
  form-action 'self';
```

### 10.4 Input Sanitization
- All HTML: DOMPurify on client AND server
- SQL: Parameterized queries ONLY (Drizzle/Prisma enforce this)
- Files: Magic bytes validation, extension whitelist, size limits, virus scan
- URLs: Strict regex, protocol whitelist (http/https only)

---

## SECTION 11: PERFORMANCE OPTIMIZATION

### 11.1 Bundle Optimization
- Route-based code splitting (Next.js automatic)
- Dynamic imports for heavy components:
  ```tsx
  const HeavyChart = dynamic(() => import("./HeavyChart"), {
    ssr: false,
    loading: () => <ChartSkeleton />
  });
  ```
- Tree shaking: ES modules, sideEffects: false in package.json
- Dependency audit: Remove unused packages monthly

### 11.2 Image Optimization
- Next.js `<Image>` component mandatory (not `<img>`)
- Formats: AVIF (priority), WebP (fallback), JPEG (legacy)
- Sizes attribute for responsive images
- Priority loading for above-fold images
- Blur placeholder for below-fold

### 11.3 Font Optimization
- `next/font` for self-hosting (no Google Fonts CDN in production)
- `font-display: swap` to prevent FOIT
- Subset to only needed characters
- Preload critical font files

### 11.4 Database Optimization
- Connection pooling: PgBouncer or ORM pool
- Query optimization: EXPLAIN ANALYZE on slow queries
- Indexing: B-tree on foreign keys, GIN on JSONB/text search
- N+1 prevention: Eager loading or DataLoader pattern
- Caching: Redis for hot data, materialized views for reports

### 11.5 Web Vitals Targets
| Metric | Target | Budget |
|--------|--------|--------|
| LCP (Largest Contentful Paint) | < 2.5s | 2.0s |
| FID (First Input Delay) | < 100ms | 50ms |
| CLS (Cumulative Layout Shift) | < 0.1 | 0.05 |
| TTFB (Time to First Byte) | < 600ms | 300ms |
| FCP (First Contentful Paint) | < 1.8s | 1.2s |
| INP (Interaction to Next Paint) | < 200ms | 150ms |

---

## SECTION 12: TESTING STRATEGY

### 12.1 Test Pyramid
```
        /\
       /  \     E2E (Playwright) — 5% of tests
      /____\    Critical user journeys
     /      \
    /--------\  Integration (Vitest + MSW) — 15%
   /          \  API contracts, database logic
  /------------\
 /              \ Unit (Vitest + React Testing Library) — 80%
/________________\ Components, utilities, hooks
```

### 12.2 Testing Requirements
- **Unit tests**: Every utility function, hook, and component
- **Component tests**: Render, user events, assertions on output
- **API tests**: Supertest for route handlers, database state verification
- **E2E tests**: Playwright, critical paths only (signup → action → payment)
- **Visual regression**: Chromatic or Percy for UI components
- **Accessibility**: axe-core automated scans
- **Performance**: Lighthouse CI in pipeline

### 12.3 Test File Structure
```
src/
├── components/
│   ├── button/
│   │   ├── button.tsx
│   │   ├── button.test.tsx
│   │   └── button.stories.tsx
├── lib/
│   ├── utils.ts
│   └── utils.test.ts
├── app/
│   └── api/
│       └── users/
│           └── route.test.ts
tests/
├── e2e/
│   ├── auth.spec.ts
│   └── checkout.spec.ts
└── fixtures/
    └── users.ts
```

### 12.4 Live Data Testing
- **Staging environment**: Mirror production with anonymized data
- **Feature flags**: LaunchDarkly for gradual rollout (1% → 10% → 50% → 100%)
- **Shadow testing**: Run new implementation alongside old, compare outputs
- **Canary deployment**: 5% traffic to new version, monitor error rates
- **Smoke tests**: Post-deployment automated checks
- **Synthetic monitoring**: Pingdom checks every 60 seconds

---

## SECTION 13: CI/CD PIPELINE

### 13.1 GitHub Actions Workflow
```yaml
name: CI/CD Pipeline

on:
  push:
    branches: [main, develop]
  pull_request:
    branches: [main]

jobs:
  lint-and-typecheck:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'
      - run: npm ci
      - run: npm run lint
      - run: npm run type-check
      - run: npm run format:check

  test:
    runs-on: ubuntu-latest
    needs: lint-and-typecheck
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'
      - run: npm ci
      - run: npm run test:unit -- --coverage
      - run: npm run test:integration
      - uses: codecov/codecov-action@v3

  build:
    runs-on: ubuntu-latest
    needs: test
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'
      - run: npm ci
      - run: npm run build
      - name: Upload build artifacts
        uses: actions/upload-artifact@v4
        with:
          name: build
          path: .next

  e2e:
    runs-on: ubuntu-latest
    needs: build
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with:
          node-version: '20'
          cache: 'npm'
      - run: npm ci
      - run: npx playwright install --with-deps
      - run: npm run test:e2e

  deploy-staging:
    if: github.ref == 'refs/heads/develop'
    needs: [build, e2e]
    runs-on: ubuntu-latest
    steps:
      - name: Deploy to Vercel (Staging)
        run: npx vercel --token=${{ secrets.VERCEL_TOKEN }} --confirm

  deploy-production:
    if: github.ref == 'refs/heads/main'
    needs: [build, e2e]
    runs-on: ubuntu-latest
    steps:
      - name: Deploy to Vercel (Production)
        run: npx vercel --prod --token=${{ secrets.VERCEL_TOKEN }} --confirm
```

### 13.2 Pre-commit Hooks (Husky + lint-staged)
```json
{
  "husky": {
    "hooks": {
      "pre-commit": "lint-staged",
      "commit-msg": "commitlint -E HUSKY_GIT_PARAMS"
    }
  },
  "lint-staged": {
    "*.{ts,tsx}": ["eslint --fix", "prettier --write"],
    "*.{json,md}": ["prettier --write"]
  }
}
```

### 13.3 Conventional Commits
```
feat:      New feature
fix:       Bug fix
docs:      Documentation only
style:     Formatting, missing semicolons, etc
refactor:  Code change that neither fixes a bug nor adds a feature
perf:      Performance improvement
test:      Adding or correcting tests
chore:     Build process or auxiliary tool changes
ci:        CI/CD changes
security:  Security-related changes
```

---

## SECTION 14: DEPLOYMENT & INFRASTRUCTURE

### 14.1 Docker Configuration
```dockerfile
# Multi-stage Dockerfile
FROM node:20-alpine AS base
RUN apk add --no-cache libc6-compat
WORKDIR /app

# Dependencies
FROM base AS deps
COPY package*.json ./
RUN npm ci --only=production

# Builder
FROM base AS builder
COPY --from=deps /app/node_modules ./node_modules
COPY . .
ENV NEXT_TELEMETRY_DISABLED=1
RUN npm run build

# Runner
FROM node:20-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
ENV NEXT_TELEMETRY_DISABLED=1
RUN addgroup --system --gid 1001 nodejs
RUN adduser --system --uid 1001 nextjs
COPY --from=builder /app/public ./public
COPY --from=builder --chown=nextjs:nodejs /app/.next/standalone ./
COPY --from=builder --chown=nextjs:nodejs /app/.next/static ./.next/static
USER nextjs
EXPOSE 3000
ENV PORT=3000
ENV HOSTNAME="0.0.0.0"
CMD ["node", "server.js"]
```

### 14.2 Environment Configuration
```env
# .env.example — copy to .env.local for development
# Database
DATABASE_URL="postgresql://user:password@localhost:5432/dbname"
REDIS_URL="redis://localhost:6379"

# Authentication
NEXTAUTH_URL="http://localhost:3000"
NEXTAUTH_SECRET="your-secret-key-min-32-chars"
GOOGLE_CLIENT_ID=""
GOOGLE_CLIENT_SECRET=""

# API Keys
OPENAI_API_KEY=""
STRIPE_SECRET_KEY=""
STRIPE_WEBHOOK_SECRET=""

# Storage
S3_BUCKET_NAME=""
S3_REGION=""
S3_ACCESS_KEY_ID=""
S3_SECRET_ACCESS_KEY=""

# Monitoring
SENTRY_DSN=""
LOGTAIL_TOKEN=""
```

### 14.3 Health Checks
```ts
// app/api/health/route.ts
export async function GET() {
  const checks = {
    database: await checkDatabase(),
    redis: await checkRedis(),
    storage: await checkStorage(),
  };
  
  const allHealthy = Object.values(checks).every(Boolean);
  
  return Response.json(
    { status: allHealthy ? "healthy" : "degraded", checks, timestamp: new Date().toISOString() },
    { status: allHealthy ? 200 : 503 }
  );
}
```

---

## SECTION 15: DOCUMENTATION REQUIREMENTS

### 15.1 README.md Structure
```markdown
# Project Name

[![CI/CD](https://github.com/user/repo/actions/workflows/ci.yml/badge.svg)](https://github.com/user/repo/actions)
[![Coverage](https://codecov.io/gh/user/repo/branch/main/graph/badge.svg)](https://codecov.io/gh/user/repo)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

> One-line description of what this project does.

[Live Demo](https://your-project.vercel.app) · [Documentation](https://docs.your-project.com) · [Report Bug](https://github.com/user/repo/issues)

## Features

- [x] Feature one with icon (Lucide: CheckCircle)
- [x] Feature two
- [x] Feature three

## Tech Stack

- **Frontend**: Next.js 15, React 19, TypeScript, Tailwind CSS, shadcn/ui
- **Backend**: tRPC, Prisma, PostgreSQL, Redis
- **Auth**: NextAuth.js v5
- **Deployment**: Vercel, Docker, GitHub Actions

## Quick Start

```bash
# 1. Clone
git clone https://github.com/user/repo.git
cd repo

# 2. Install
npm install

# 3. Environment
cp .env.example .env.local
# Edit .env.local with your credentials

# 4. Database
npx prisma migrate dev
npx prisma db seed

# 5. Run
npm run dev
```

## Architecture

[Insert architecture diagram — Mermaid or SVG]

## API Documentation

Auto-generated at `/api/docs` (Swagger UI)

## Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md)

## License

MIT © [Your Name](https://github.com/username)
```

### 15.2 Additional Documentation Files
- `ARCHITECTURE.md` — System design decisions, ADRs
- `CONTRIBUTING.md` — PR template, code style, setup guide
- `SECURITY.md` — Reporting vulnerabilities, security practices
- `CHANGELOG.md` — Keep a Changelog format
- `docs/API.md` — Endpoint reference with examples
- `docs/DEPLOYMENT.md` — Step-by-step deployment guide

---

## SECTION 16: PROJECT-SPECIFIC ADAPTATIONS

### 16.1 E-Commerce Websites
Additional requirements:
- Product catalog with faceted search (Algolia/Meilisearch)
- Shopping cart (Redis-backed, persistent across sessions)
- Checkout flow (Stripe integration, webhook handling)
- Inventory management (real-time stock updates)
- Order tracking (status workflow, email notifications)
- Admin dashboard (sales analytics, inventory management)
- Multi-currency support (Exchange rate API)
- Tax calculation (TaxJar or Stripe Tax)

### 16.2 SaaS Dashboards
Additional requirements:
- Multi-tenancy (row-level isolation or schema-per-tenant)
- Subscription management (Stripe Billing, portal integration)
- Team/organization support (invitations, roles, permissions)
- Usage tracking (metered billing, quota enforcement)
- Webhook management (user-configurable endpoints)
- API key generation (scoped, revocable)
- Audit logs (immutable, exportable)
- Feature flags per plan tier

### 16.3 Content-Heavy Sites
Additional requirements:
- CMS integration (Sanity, Strapi, or headless WordPress)
- SEO optimization (meta tags, structured data, sitemap.xml, robots.txt)
- Content preview (draft mode, scheduled publishing)
- Media management (image optimization, CDN, video streaming)
- Search functionality (full-text + fuzzy matching)
- RSS feeds (Atom + RSS 2.0)
- Social sharing (Open Graph, Twitter Cards)
- Comments system (Giscus or custom with moderation)

### 16.4 Real-Time Applications
Additional requirements:
- WebSocket architecture (Socket.io or native WS)
- Presence indicators (who's online, typing indicators)
- Collaborative editing (Yjs or Operational Transform)
- Live notifications (SSE for server push)
- Real-time analytics (WebSocket or polling fallback)
- Connection resilience (auto-reconnect, state sync)

---

## SECTION 17: FINAL DELIVERY CHECKLIST

Before marking any webapp project as complete, verify:

### Code Quality
- [ ] TypeScript strict mode enabled, zero `any` types
- [ ] ESLint passes with zero warnings
- [ ] Prettier formatting applied to all files
- [ ] No console.log statements in production code
- [ ] All TODO comments resolved or ticketed
- [ ] Minimum 10,000 lines of production code (excluding node_modules, tests, docs)
- [ ] No emoji characters anywhere in codebase — verified with grep

### Functionality
- [ ] All features from requirements implemented
- [ ] Responsive design tested on actual devices (not just Chrome DevTools)
- [ ] Dark mode toggle works across all pages
- [ ] All forms validate client-side and server-side
- [ ] Error boundaries catch and log all React errors
- [ ] Loading states present for all async operations
- [ ] Empty states designed for all data-dependent views

### Performance
- [ ] Lighthouse score 90+ on all metrics
- [ ] Bundle size under 200KB initial JS (gzipped)
- [ ] All images optimized (WebP/AVIF with fallbacks)
- [ ] Fonts self-hosted and subset
- [ ] No render-blocking resources
- [ ] Core Web Vitals pass on mobile and desktop

### Security
- [ ] All API routes protected with authentication where required
- [ ] CSRF tokens on all state-changing forms
- [ ] XSS prevention (CSP headers, output encoding)
- [ ] Rate limiting active on all public endpoints
- [ ] Secrets in environment variables only, never committed
- [ ] Dependency audit passes (`npm audit`)
- [ ] HTTPS enforced (HSTS header)

### Testing
- [ ] Unit test coverage > 70%
- [ ] Integration tests for all API routes
- [ ] E2E tests for critical user journeys
- [ ] All tests passing in CI
- [ ] Manual QA completed on staging

### DevOps
- [ ] Docker image builds successfully
- [ ] CI/CD pipeline green on main branch
- [ ] Staging deployment tested and approved
- [ ] Production deployment successful
- [ ] Health checks responding 200 OK
- [ ] Error monitoring (Sentry) receiving events
- [ ] Analytics tracking implemented

### Documentation
- [ ] README.md complete with quick-start GIF
- [ ] API documentation generated and accessible
- [ ] Environment variables documented in .env.example
- [ ] Architecture diagram included
- [ ] Changelog updated with version notes
- [ ] License file present

---

## MANDATORY OUTPUT SEQUENCE

When generating a website/webapp for a user, produce deliverables in this order:

1. **Analysis** — Project type, aim, user personas, tech-stack justification
2. **Architecture** — System diagram, database schema, API design
3. **Design System** — Color tokens, typography, component inventory, animation specs
4. **Project Structure** — Complete folder tree with file purposes
5. **Core Implementation** — Configuration files, database schema, API routes, components
6. **Feature Implementation** — Page-by-page, feature-by-feature code
7. **Testing Suite** — Unit, integration, and E2E test files
8. **DevOps Configuration** — Docker, CI/CD, deployment scripts
9. **Documentation** — README, API docs, architecture decisions
10. **Deployment Guide** — Step-by-step production deployment instructions

**REMEMBER**: 
- NO EMOJIS — use Lucide icons, Unicode symbols, or Font Awesome
- MINIMUM 10,000 lines of code for production-grade delivery
- EVERY instruction above must be visibly satisfied in the output
- When in doubt, over-engineer rather than under-deliver

---

*End of GOVINDA CLI Website & Webapp Protocol v2.0*