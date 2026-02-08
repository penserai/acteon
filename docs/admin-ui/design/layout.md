# Acteon Admin UI -- App Shell & Layout

> Overall application structure, responsive behavior, and layout constants.

---

## App Shell

The application uses an **Inverted-L layout** (following Linear's pattern): a persistent sidebar on the left and a header bar across the top, with the content area filling the remainder.

```
+--------+-----------------------------------------------------------+
|        |  [Breadcrumbs]                  [Theme] [Cmd+K] [User]   |  56px header
|        |-----------------------------------------------------------+
|        |                                                           |
|  Side  |                                                           |
|  bar   |                    Content Area                           |
|        |                   (fluid width)                           |
| 240px  |                  max-width: 1440px                        |
|  or    |                    centered                               |
| 64px   |                   padding: 24px                           |
|        |                                                           |
|        |                                                           |
+--------+-----------------------------------------------------------+
```

---

## Sidebar

| Property | Expanded | Collapsed |
|----------|----------|-----------|
| Width | 240px | 64px |
| Content | Icons + text labels | Icons only (tooltips on hover) |
| Logo | "Acteon" wordmark | "A" icon mark |
| Section dividers | Visible with labels | Hidden |
| Count badges | Visible | Dot indicator only |
| User section | Avatar + name + logout | Avatar only |

**Collapse trigger**: Toggle button at top of sidebar. Keyboard shortcut: `Cmd+[` (Mac) / `Ctrl+[`.

**Persistence**: Collapse state saved to `localStorage`. Defaults to expanded on first visit.

**Sections**:

```
MAIN
  Dashboard
  Dispatch
  Rules
  Audit Trail
  Events
  Groups
  Chains
  Approvals         [count badge if > 0]
  Circuit Breakers
  Dead-Letter Queue  [! badge if count > 0]
  Stream
  Embeddings

SETTINGS
  Rate Limiting
  Auth & Users
  Providers
  LLM Guardrail
  Telemetry
  Server Config
  Background Tasks
```

**Ordering rationale**: Items are ordered by usage frequency (dispatch-related views first, configuration last), following Datadog's sidebar organization principle.

**Role-based visibility**: Menu items hidden based on user role (see Appendix E of the UI specification). Hidden items are not rendered at all (not disabled).

---

## Header Bar

| Property | Value |
|----------|-------|
| Height | 56px |
| Background | `surface-primary` with bottom border `border-default` |
| Position | Fixed, full width minus sidebar |

**Contents (left to right)**:

```
[Breadcrumbs]                               [Theme toggle] [Cmd+K hint] [User dropdown]
```

- **Breadcrumbs**: Auto-generated from route hierarchy (e.g., `Chains / chain-abc-123 / Steps`)
- **Theme toggle**: Sun/moon icon button. Cycles: System -> Light -> Dark
- **Cmd+K hint**: Subtle button showing `Cmd+K` shortcut, clicking opens command palette
- **User dropdown**: Avatar + name. Dropdown: role badge, switch theme, keyboard shortcuts reference, logout

---

## Content Area

| Property | Value |
|----------|-------|
| Width | Fluid, `max-width: 1440px`, centered horizontally |
| Padding | 24px (`space-6`) all sides |
| Background | `surface-secondary` (slightly tinted to differentiate from cards) |
| Scroll | Vertical scroll, independent of sidebar |

**Content structure per page**:

```
[Page Header]        -- Title, description, action buttons
[Filter Bar]         -- Active filters, search, view toggles (if applicable)
[Main Content]       -- Cards, tables, visualizations
[Pagination]         -- If applicable
```

---

## Responsive Breakpoints

| Breakpoint | Width | Layout Changes |
|------------|-------|----------------|
| Desktop | > 1024px | Full layout: expanded/collapsed sidebar + header + content |
| Tablet | 768px - 1024px | Sidebar collapsed by default (64px), content fills remainder. Side panels overlay instead of pushing. |
| Mobile | < 768px | Sidebar becomes bottom navigation bar (5 primary items) or hamburger drawer. Header simplified. Side panels become full-screen overlays. Tables switch to card-based list view. |

### Desktop (> 1024px)

Full experience as described above. Side panels push content or overlay depending on context (user preference).

### Tablet (768px - 1024px)

```
+------+-----------------------------------------------+
| 64px |                                               |
| side |              Content Area                     |
| bar  |            (fluid, centered)                  |
|      |                                               |
+------+-----------------------------------------------+
```

- Sidebar always collapsed (icon-only)
- Side panels (drawers) overlay with backdrop, do not push
- Stat cards grid: 2 columns instead of 4
- DAG visualizer: zoom out to fit, horizontal scrolling enabled
- Tables: horizontal scroll if columns exceed width

### Mobile (< 768px)

```
+-----------------------------------------------+
| [hamburger]  Acteon         [Theme] [User]    |  48px header
|-----------------------------------------------|
|                                               |
|              Content Area                     |
|              (full width, 16px padding)       |
|                                               |
|-----------------------------------------------|
| [Dash] [Rules] [Chains] [Audit] [More...]    |  56px bottom nav
+-----------------------------------------------+
```

- **Header**: Simplified to 48px. Hamburger icon opens full-screen sidebar drawer.
- **Bottom navigation**: 5 primary items (Dashboard, Rules, Chains, Audit, More). "More" opens the full navigation list.
- **Tables**: Switch to card-based list view -- each row becomes a card with key fields stacked vertically.
- **Side panels**: Become full-screen overlays with back button.
- **Command palette**: Full-screen overlay.
- **DAG visualizer**: Compact list view with step cards, visual edges hidden (replaced by step sequence numbers).
- **Content padding**: Reduced to 16px (`space-4`).
- **Touch targets**: All interactive elements minimum 44x44px.

---

## Page Layout Templates

### List View Template (Audit Trail, Rules, Chains, Events, Groups, Approvals, DLQ, Scheduled)

```
+----------------------------------------------------------------------+
| [breadcrumbs]                                                        |
| Page Title                                 [Primary Action]          |
|----------------------------------------------------------------------|
| [Search]  [Filter 1 v] [Filter 2 v] [Filter 3 v]  [Clear filters] |
|----------------------------------------------------------------------|
| [ Data Table ]                                                       |
|                                                                      |
|                                                                      |
|                                                                      |
|----------------------------------------------------------------------|
| Showing 1-50 of 2,847          [< 1 2 3 ... 57 >]                  |
+----------------------------------------------------------------------+
```

### Detail View Template (via Side Panel)

```
+----------------------------------------------------------------------+
| [ List View stays visible ]           | Detail Panel (440px)         |
|                                       |------------------------------|
|                                       | [Title]           [x] [>>]  |
|                                       | [Tabs]                       |
|                                       |                              |
|                                       | [Content sections]           |
|                                       |                              |
|                                       |          [Actions]           |
+----------------------------------------------------------------------+
```

`[>>]` button opens full-page detail view.

### Dashboard Template

```
+----------------------------------------------------------------------+
| [breadcrumbs]                                                        |
| Dashboard                           [Time range v]  [Refresh]       |
|----------------------------------------------------------------------|
| [Stat] [Stat] [Stat] [Stat] [Stat] [Stat] [Stat] [Stat]           |
|----------------------------------------------------------------------|
| [                 Time-series chart (full width)                 ]   |
|----------------------------------------------------------------------|
| [  Provider Health Cards  ]    |    [ Recent Activity Feed       ]  |
| [  (2-column grid)        ]    |    [ (scrolling event list)     ]  |
+----------------------------------------------------------------------+
```

### Split Pane Template (Rule Editor)

```
+----------------------------------------------------------------------+
| [breadcrumbs]                                                        |
| Edit Rule: block-large-emails        [Save]  [Discard]  [Delete]   |
|----------------------------------------------------------------------|
| [Code Editor (50%)]     |divider|    [Preview + Test (50%)]         |
|                          |      |                                    |
|                          |      |                                    |
+----------------------------------------------------------------------+
```

### DAG View Template (Chain Detail)

```
+----------------------------------------------------------------------+
| [breadcrumbs]                                                        |
| Chain: process-order  [Running badge]               [Cancel Chain]  |
|----------------------------------------------------------------------|
| [                  DAG Visualizer (full width)                   ]   |
| [                  (zoom/pan/fit controls top-right)             ]   |
|                                                                      |
|----------------------------------------------------------------------|
| [Step Detail Panel]    -- appears below or as side panel on click    |
+----------------------------------------------------------------------+
```

---

## Deep Linking

Every significant UI state is encoded in the URL following Vercel's deep-linking philosophy:

| State | URL Encoding |
|-------|-------------|
| Active page | Path segment: `/chains`, `/audit` |
| Selected item | Path segment: `/chains/abc-123` |
| Active tab | Query param: `?tab=steps` |
| Filters | Query params: `?namespace=prod&outcome=failed` |
| Pagination | Query params: `?page=3&limit=50` |
| Sort | Query params: `?sort=dispatched_at&order=desc` |
| Side panel open | Query param: `?detail=abc-123` |
| Time range | Query params: `?from=2026-02-07T00:00:00Z&to=2026-02-08T00:00:00Z` |
| Theme | `localStorage` only (not URL) |
| Sidebar state | `localStorage` only (not URL) |

This allows operators to share exact views by copying the URL -- critical for incident triage and collaboration.
