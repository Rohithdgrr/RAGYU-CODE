# GOVINDA PROJECT PLAN TEMPLATE

The GOVINDA Protocol enforcement mechanism reads this file and appends
it to the model’s system message whenever `enforce_protocol = true`.
The plan is what the model is required to produce **before** writing
any implementation code.

You can edit this file freely — it is read at session startup and the
user is encouraged to tailor it to their team’s conventions. If the
file is missing, a built-in stub is used as a fallback.

---

## SECTION A: PROJECT ANALYSIS

### A1. Aim & Problem Statement
- **Mission**: [One sentence]
- **Problem**: [What pain does this solve?]
- **Target Audience**: [Primary, secondary personas]
- **Success Metrics**: [Measurable outcomes]
- **Competitive Analysis**: [3 competitors + their weaknesses]

### A2. Scope Declaration
THIS IS A PRODUCTION-GRADE PROJECT. Not a prototype. Not an MVP. Not a tutorial.
- Minimum Scale: 10,000+ lines of production code
- Quality Standard: Enterprise-grade
- Compliance: WCAG 2.1 AA, GDPR-ready, security-hardened

### A3. Input / Output Matrix
| Input | Format | Validation | Handling |
|-------|--------|------------|----------|
| [List all inputs] | | | |

| Output | Format | Optimization | Fallback |
|--------|--------|--------------|----------|
| [List all outputs] | | | |

### A4. Working Mechanism
- Data Flow: [Step-by-step]
- State Machine: [States and transitions]
- Concurrency Model: [Threads, async, actors]
- Error Strategy: [Fail-fast vs graceful degradation]

---

## SECTION B: ARCHITECTURE & TECH STACK

### B1. Tech Stack (Every Layer Justified)
| Layer | Technology | Justification |
|-------|-----------|---------------|
| Frontend | | |
| Backend | | |
| Database | | |
| Cache | | |
| Queue | | |
| Search | | |
| DevOps | | |
| AI/ML | | (if applicable) |

### B2. System Architecture
```
[Insert textual architecture diagram]
```

### B3. Database Schema
- All tables with: UUID primary keys (v7), created_at / updated_at
  timestamps, soft deletes (deleted_at), proper indexing, RLS
  policies (if applicable), foreign keys with ON DELETE rules.

### B4. API Design
- **Base Path**: `/api/v1/`
- **Authentication**: [JWT / OAuth2 / API Keys]
- **Rate Limiting**: [Tiered strategy]
- **Pagination**: [Cursor vs Offset]
- **Error Format**: RFC 7807 Problem Details
- **Endpoints**: [Complete list with methods, params, responses]

### B5. Security Architecture
- Auth Flow: [Diagram]
- Authorization: [RBAC / ABAC matrix]
- Input Validation: [Zod / Joi / serde schemas]
- Output Encoding: [XSS prevention]
- Secrets Management: [Keychain / Vault / ENV]
- Dependency Scanning: [npm audit / cargo audit]

### B6. Performance Budget
| Metric | Target | Budget |
|--------|--------|--------|
| LCP | < 2.5s | 2.0s |
| API P95 | < 200ms | 150ms |
| Bundle Size | < 200KB | 150KB |
| Query Time | < 10ms | 5ms |

---

## SECTION C: DESIGN SYSTEM

### C1. Visual Tokens
- **Colors**: Full CSS variable list (light + dark)
- **Typography**: Full scale (xs to 7xl) with font families
- **Spacing**: 4px base grid (space-1 to space-64)
- **Shadows**: 5 elevation levels
- **Radii**: Full scale (sm to full)

### C2. Iconography
- **Source**: Lucide / Phosphor / SF Symbols / Material
- **Sizes**: 16px (inline), 20px (buttons), 24px (nav), 32px (empty states)
- **Rule**: NO EMOJI — custom vector icons or icon font only

### C3. Component Inventory
- [ ] Navigation (sidebar, topbar, breadcrumbs, command palette)
- [ ] Forms (all input types, validation, error states)
- [ ] Data Display (table, cards, charts, calendar, timeline)
- [ ] Feedback (toast, modal, alert, progress, skeleton, empty state)
- [ ] Overlays (modal, drawer, dropdown, tooltip, popover)

### C4. Animation System
- **Durations**: 0ms, 150ms, 200ms, 300ms, 500ms
- **Easings**: Default, Enter, Exit, Bounce
- **Patterns**: Page transitions, micro-interactions, loading states,
  scroll effects
- **Accessibility**: `prefers-reduced-motion` support

### C5. Responsive Breakpoints
| Name | Width | Layout |
|------|-------|--------|
| xs | < 640px | Single column, stacked nav |
| sm | 640px | Two column where needed |
| md | 768px | Sidebar collapses |
| lg | 1024px | Full sidebar, multi-pane |
| xl | 1280px | Maximum content width |
| 2xl | 1536px+ | Ultra-wide optimizations |

---

## SECTION D: DEVELOPMENT PLAN

### Phase 1: Foundation (Steps 1-5)
1. Project scaffolding with correct folder structure
2. Tooling: TypeScript strict, ESLint, Prettier, lint-staged, Husky
3. Design system setup: CSS variables, Tailwind config, theme provider
4. Database setup: Schema, migrations, ORM, connection pooling
5. Auth system: Login, register, session, middleware, guards

### Phase 2: Core Infrastructure (Steps 6-10)
6. API client setup: Typed requests, error handling, retry logic
7. State management: Server state (TanStack Query) + Client state (Zustand)
8. Routing and navigation: Protected routes, breadcrumbs, deep linking
9. Layout components: Sidebar, header, footer, page wrappers
10. Core utilities: Formatters, validators, hooks, helpers

### Phase 3: Feature Implementation (Steps 11-20)
[Feature-by-feature breakdown with acceptance criteria]

### Phase 4: Backend Implementation (Steps 21-30)
[Endpoint-by-endpoint breakdown with validation and tests]

### Phase 5: Integration & Real-Time (Steps 31-35)
[WebSocket/SSE setup, sync logic, file handling]

### Phase 6: Testing (Steps 36-45)
- Unit tests: 70%+ coverage
- Integration tests: API contract tests
- E2E tests: Critical user journeys
- Visual regression: Chromatic/ Percy
- Performance: Lighthouse CI, k6 load tests

### Phase 7: DevOps & Deployment (Steps 46-50)
- Docker: Multi-stage build, non-root user
- CI/CD: GitHub Actions (lint, test, build, deploy)
- Monitoring: Sentry, analytics, health checks
- Documentation: README, API docs, architecture, contributing

---

## SECTION E: EXPANSION CHECKLIST

To reach 10,000+ lines, ensure these are included:

- [ ] Admin dashboard with analytics
- [ ] User roles and permissions (RBAC)
- [ ] Data export (JSON, CSV, PDF, Excel)
- [ ] Import functionality with validation
- [ ] Advanced search (full-text + filters + sorting)
- [ ] Real-time notifications
- [ ] Email integration (transactional)
- [ ] File upload and management
- [ ] Activity logs and audit trail
- [ ] API rate limiting and usage analytics
- [ ] Webhook system
- [ ] Feature flags
- [ ] A/B testing infrastructure
- [ ] Multi-tenancy support
- [ ] Internationalization (i18n)
- [ ] Accessibility audit (WCAG 2.1 AA)
- [ ] Performance optimization (lazy loading, virtualization, memoization)
- [ ] Security hardening (CSP, CSRF, XSS, SQL injection prevention)
- [ ] Comprehensive error handling with user-friendly messages
- [ ] Onboarding flow and tooltips
- [ ] Keyboard shortcuts and command palette
- [ ] Dark mode + system theme detection
- [ ] Mobile-responsive design
- [ ] PWA support (service worker, manifest, offline)
- [ ] SEO optimization (SSG, meta tags, sitemap)
- [ ] Backup and restore functionality
- [ ] Data migration scripts
- [ ] CLI tooling for admin tasks
- [ ] Comprehensive logging and monitoring
- [ ] Load testing and stress testing scripts

---

## SECTION F: QUALITY GATES

Before marking ANY phase complete:

- [ ] All code is typed (strict TypeScript / Rust types / Python type hints)
- [ ] All public APIs documented
- [ ] All features have tests
- [ ] All states handled (loading, error, empty, success)
- [ ] All inputs validated (client + server)
- [ ] All outputs encoded safely
- [ ] No emojis anywhere (verified with regex scan)
- [ ] Responsive at all breakpoints
- [ ] Accessible (keyboard nav, screen reader, focus indicators)
- [ ] Performance targets met

The model MUST call the `quality_gate_check` tool with
`phase=FINAL_VALIDATION` before claiming completion. The tool returns
`ALL QUALITY GATES PASSED` only when every check is true AND the
line_count_estimate meets the 10,000-line minimum.

---

## SECTION G: DELIVERY CONTRACT

I commit to:

1. Reading ALL system instructions before coding.
2. Creating this plan BEFORE implementation.
3. Executing EVERY phase to completion.
4. NEVER stopping early or delivering stubs.
5. Over-delivering on every requirement.
6. Validating against every quality gate.
7. Calling `quality_gate_check` before claiming completion.

**Status**: [ ] Plan Complete → [ ] Phase 1 → [ ] Phase 2 → [ ] Phase 3
        → [ ] Phase 4 → [ ] Phase 5 → [ ] Phase 6 → [ ] Phase 7
        → [ ] Final Validation
