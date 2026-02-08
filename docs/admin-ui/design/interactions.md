# Acteon Admin UI -- Interactions & Motion Design

> Every animation, transition, and micro-interaction in the Acteon Admin UI.
> All durations and easing curves reference tokens from the design system.

---

## Table of Contents

1. [Page Transitions](#page-transitions)
2. [Side Panel](#side-panel)
3. [Modal Dialog](#modal-dialog)
4. [Table Interactions](#table-interactions)
5. [Button Feedback](#button-feedback)
6. [Toast Notifications](#toast-notifications)
7. [Loading States](#loading-states)
8. [Circuit Breaker Animations](#circuit-breaker-animations)
9. [DAG Visualizer Animations](#dag-visualizer-animations)
10. [Real-Time Data Animations](#real-time-data-animations)
11. [Command Palette](#command-palette)
12. [Status & Badge Transitions](#status--badge-transitions)
13. [Sparkline & Chart Animations](#sparkline--chart-animations)
14. [Keyboard & Focus Indicators](#keyboard--focus-indicators)

---

## Page Transitions

**Trigger**: Navigating between sidebar routes (e.g., Dashboard -> Rules).

**Animation**:
- Outgoing content: `opacity 1 -> 0`, `translateY 0 -> -4px`, 100ms `ease-in`
- Incoming content: `opacity 0 -> 1`, `translateY 4px -> 0`, 200ms `ease-out`
- Total perceived transition: ~200ms
- Sidebar and header remain static (no transition)

**Implementation**: React transition group or `framer-motion` `AnimatePresence` wrapping the page content `<Outlet>`.

**Rationale**: The slight vertical shift provides directional context without being distracting. Sub-200ms keeps navigation feeling instant.

---

## Side Panel

**Trigger**: Clicking a table row to inspect detail, or explicit "View Detail" action.

### Opening
- Panel slides from right: `translateX(100%) -> translateX(0)`, 300ms `ease-out`
- If overlaid: backdrop fades in `opacity 0 -> 0.5`, 200ms `ease-out`
- Content area does not shift (panel overlays or pushes depending on viewport width)

### Closing
- Panel slides right: `translateX(0) -> translateX(100%)`, 200ms `ease-in`
- Backdrop fades out: `opacity 0.5 -> 0`, 200ms `ease-in`
- Triggered by: `Escape` key, clicking backdrop, clicking close button

### Panel Content Transitions
- When switching between items (clicking a different table row while panel is open):
  - Content cross-fades: outgoing `opacity 1 -> 0` (100ms), incoming `opacity 0 -> 1` (150ms)
  - Panel frame stays in place (no slide)

---

## Modal Dialog

**Trigger**: Confirmation dialogs, dispatch form, replay confirmation.

### Opening
- Backdrop: `opacity 0 -> 0.5`, 200ms `ease-out`
- Dialog: `opacity 0 -> 1` + `scale(0.95) -> scale(1)`, 200ms `ease-out` with `spring` easing
- Focus immediately moves to first focusable element (or close button)

### Closing
- Dialog: `opacity 1 -> 0` + `scale(1) -> scale(0.95)`, 150ms `ease-in`
- Backdrop: `opacity 0.5 -> 0`, 150ms `ease-in`
- Focus returns to the element that triggered the modal

---

## Table Interactions

### Row Hover
- Background: `transparent -> gray-50 dark:gray-900`, 150ms `ease-out`
- No delay on enter, 100ms delay on leave (prevents flicker when scanning rows)

### Row Selection
- Checkbox scales: `scale(0.9) -> scale(1)`, 100ms `ease-out` on check
- Selected row background: `primary-50/30 dark:primary-500/5`, fade-in 150ms

### Column Sort
- Click header: sort indicator arrow rotates 180deg if switching direction, 200ms `ease-out`
- Column header text briefly bolds (font-weight `500 -> 600 -> 500`, 200ms)
- Table content cross-fades during re-sort: `opacity 1 -> 0.5 -> 1`, 300ms

### Row Expansion
- Expanded content slides down: `height 0 -> auto` with `max-height` transition, 200ms `ease-out`
- Chevron icon rotates: `rotate(0) -> rotate(90deg)`, 200ms `ease-out`

### Pagination
- Content cross-fades between pages: `opacity 1 -> 0.7 -> 1`, 200ms
- No scroll-to-top (user maintains scroll position awareness)

---

## Button Feedback

### Hover
- Background color transition: 150ms `ease-out`
- Cursor changes to `pointer`

### Press (Active)
- `transform: scale(0.98)`, 50ms `ease-in`
- Background darkens one shade
- Release: `scale(1)`, 100ms `ease-out`

### Focus (Visible)
- Focus ring appears: `shadow-ring` (`0 0 0 2px primary-400`), 100ms `ease-out`
- Only on `:focus-visible` (keyboard navigation), not on click

### Loading State Transition
- Label text cross-fades to spinner + "Loading...", 150ms
- Button becomes disabled (no hover effects)
- On completion: spinner cross-fades back to label, 150ms
- Success feedback: brief green flash on button border (200ms)

### Danger Button Confirmation Pattern
- First click: no immediate action. Button text changes to "Confirm?" with `error-500` background pulse.
- Second click within 3s: executes the action.
- If no second click within 3s: reverts to original state.
- Alternative for critical actions: use Confirmation Dialog instead.

---

## Toast Notifications

### Entrance
- Slide in from top-right: `translateX(100%) -> translateX(0)`, 300ms `ease-out`
- Slight upward shift: `translateY(8px) -> translateY(0)` with opacity fade-in
- If multiple toasts: existing toasts shift down with 200ms transition to make room

### Auto-Dismiss
- Progress bar at bottom of toast shrinks from 100% to 0% over 5s (linear)
- Pauses on hover (progress bar stops)
- At 0%: toast fades out

### Manual Dismiss
- Click close button: toast slides right `translateX(0) -> translateX(100%)`, 200ms `ease-in`
- Remaining toasts shift up to fill gap, 200ms `ease-out`

### Stacking
- Max 5 visible toasts
- New toasts push from top; oldest at bottom
- Beyond 5: oldest auto-dismissed immediately

---

## Loading States

### Skeleton Shimmer
- Background gradient sweeps left-to-right: `linear-gradient(90deg, gray-100 25%, gray-50 50%, gray-100 75%)`
- Animation: `translateX(-100%) -> translateX(100%)`, 1.5s `ease-in-out`, infinite loop
- Dark mode: `gray-800 -> gray-700 -> gray-800`

### Spinner Display Timing
Following Vercel's research:
1. **0-200ms**: No visual indicator (operation may complete before user notices)
2. **200ms+**: Fade in spinner over 150ms
3. **Once shown**: Display for minimum 400ms even if operation completes earlier
4. **On completion**: Cross-fade from spinner to content, 200ms

### Optimistic Updates
- For toggle operations (rule enable/disable): update UI immediately, revert on error with error toast
- For dispatch: show "Dispatching..." state, then replace with result

### Full-Page Loading
- Used only on initial app load or authentication redirect
- Centered spinner with "Acteon" wordmark below, fade-in after 500ms

---

## Circuit Breaker Animations

### State Indicator
- **Closed (healthy)**: Steady green filled circle
- **Open (rejecting)**: Red filled circle with slow pulse animation (`opacity 1 -> 0.6 -> 1`, 2s, infinite)
- **Half-Open (testing)**: Amber half-filled circle with gentle blink (`opacity 1 -> 0.8 -> 1`, 1.5s, infinite)

### State Transition Animation
When circuit state changes (e.g., Closed -> Open):
1. Current state node briefly scales up: `scale(1) -> scale(1.15)`, 200ms
2. Transition arrow pulses: `stroke-width 1 -> 3 -> 1`, `opacity 0.5 -> 1 -> 0.5`, 400ms
3. New state node pulses once: `scale(0.9) -> scale(1.1) -> scale(1)`, 300ms `spring`
4. Color transitions smoothly between states: 400ms `ease-out`
5. Toast notification fires simultaneously

### State Diagram Interaction
- Hover on state node: node lifts with `shadow-md`, tooltip shows details
- Hover on transition arrow: arrow thickens, label appears
- Click on active state: opens detail panel

---

## DAG Visualizer Animations

### Initial Render
- Nodes fade in sequentially (50ms stagger per node), left-to-right, top-to-bottom
- Edges draw with SVG `stroke-dashoffset` animation, 300ms per edge after source node appears
- Total render: ~500ms for a typical 5-step chain

### Active Step Pulse
- The currently executing step node has a soft glow animation:
  - Box shadow: `0 0 0 4px primary-400/30` pulsing `opacity 0.3 -> 0.7 -> 0.3`, 2s infinite
  - Node border: subtle `primary-400` color pulse

### Execution Path Highlight
- When execution completes a step:
  1. Completed node color transitions from `info-500` (in-progress) to `success-500` (completed), 400ms
  2. Edge from completed node to next node "draws" with `stroke-dashoffset` animation, 300ms
  3. Next node transitions from `gray-400` (pending) to `info-500` (in-progress), 200ms
  4. New active node begins pulse animation

### Branch Taken Animation
- When a branch condition evaluates:
  1. Taken edge: animates to `stroke-width: 3`, `stroke: primary-400`, 200ms
  2. Not-taken edges: fade to `opacity: 0.3`, `stroke: gray-300`, 200ms
  3. Branch condition label on taken edge: background flashes `success-50`, 300ms

### Failed Step Animation
- Node background transitions to `error-500`, border to `error-700`, 300ms
- Subtle shake animation: `translateX(0) -> (-2px) -> (2px) -> (-1px) -> (1px) -> 0`, 300ms
- Error icon fades in at top-right of node

### Zoom and Pan
- Zoom: Smooth `transform: scale()` transition, 200ms per zoom step
- Pan: Follows cursor/touch with no delay (immediate transform)
- Fit-to-view button: Smooth zoom + pan to center all nodes, 400ms `ease-out`

---

## Real-Time Data Animations

### Counter Increment (Dashboard Stat Cards)
- When a metric value increases:
  - Number "ticks up" with counting animation: old value -> new value over 500ms
  - Uses `requestAnimationFrame` for smooth interpolation
  - Brief color flash: text changes to `primary-400` for 300ms, then fades back to default

### Activity Feed New Event
- New event at top: `height 0 -> auto` + `opacity 0 -> 1`, 200ms `ease-out`
- If auto-scrolling: smooth scroll to top, 150ms
- New event background briefly highlights: `primary-50/50` -> `transparent`, 1s fade

### SSE Connection Status
- **Connected**: Green dot, steady
- **Reconnecting**: Yellow dot, blink animation (`opacity 0 -> 1`, 500ms, infinite)
- **Disconnected**: Red dot with crossed-out icon. "Reconnect" button appears with fade-in, 200ms

### "New Events" Badge
- When user scrolls down and new events arrive:
  - Badge slides down from top of feed: `translateY(-100%) -> translateY(0)`, 200ms
  - Shows count: "12 new events"
  - Click scrolls to top and fades badge out, 200ms

---

## Command Palette

### Opening (Cmd+K)
- Backdrop fades in: `opacity 0 -> 0.5`, 150ms `ease-out`
- Panel scales in: `opacity 0 -> 1` + `scale(0.95) -> scale(1)` + `translateY(-8px) -> translateY(0)`, 200ms `spring`
- Input auto-focuses immediately
- Recent items fade in: 100ms stagger per item (up to 5 items)

### Closing
- Panel: `opacity 1 -> 0` + `scale(1) -> scale(0.95)`, 150ms `ease-in`
- Backdrop: `opacity 0.5 -> 0`, 150ms `ease-in`
- Focus returns to previously focused element

### Result Filtering
- Results update immediately as user types (no debounce)
- New results fade in: `opacity 0 -> 1`, 100ms
- Removed results: instant removal (no exit animation -- speed is paramount)

### Keyboard Navigation
- Active item: background `primary-50 dark:primary-500/10`, transitions between items with 50ms `ease-out`
- Arrow keys: immediate highlight transition
- Enter on item: brief `scale(0.98)` press feedback before palette closes

---

## Status & Badge Transitions

### Status Badge Color Change
- When a status changes (e.g., chain step "pending" -> "completed"):
  - Badge background and text color cross-fade: 300ms `ease-out`
  - Brief scale pop: `scale(1) -> scale(1.05) -> scale(1)`, 200ms

### Toggle Switch
- Thumb slides: `translateX(0) -> translateX(16px)`, 150ms `ease-out`
- Track color transitions: `gray-300 -> primary-400`, 150ms `ease-out`
- On error (toggle fails): thumb bounces back with `spring` easing, error toast appears

### Rule Enabled/Disabled Toggle
- Optimistic: toggle moves immediately
- Row opacity: disabled rows transition to `opacity 0.6`, 200ms
- On server error: toggle reverses with spring animation + error toast

---

## Sparkline & Chart Animations

### Sparkline Draw
- On initial render: SVG path draws from left to right using `stroke-dashoffset` animation, 600ms `ease-out`
- On data update: path morphs smoothly to new shape, 300ms `ease-out`

### Time-Series Chart
- Initial load: bars or lines draw from bottom-up / left-right, 500ms staggered
- Data point hover: vertical crosshair line appears instantly, tooltip fades in 100ms
- Time range change: chart cross-fades between old and new data, 300ms
- Live update: new data point slides in from right, chart shifts left, 200ms

### Donut/Pie Chart
- Initial render: arcs draw clockwise from 12 o'clock, 500ms `ease-out`
- Hover segment: segment shifts outward 4px, tooltip appears 100ms
- Value change: arcs resize smoothly, 300ms `ease-out`

---

## Keyboard & Focus Indicators

### Focus Ring
- Appears on `:focus-visible` only (not on mouse click)
- Ring: `0 0 0 2px surface-primary, 0 0 0 4px primary-400` (2px gap + 2px ring)
- Fade in: 100ms `ease-out`
- Removed immediately on blur (no fade-out)

### Skip Link
- "Skip to content" link: hidden by default, appears on first Tab press
- Positioned top-center: `translateY(-100%) -> translateY(0)`, 200ms

### Keyboard Shortcut Feedback
- When a keyboard shortcut fires (e.g., Cmd+K):
  - Brief visual echo: small tooltip near trigger point showing shortcut, 500ms then fades

---

## Performance Guidelines

### Animation Principles
1. Only animate `transform` and `opacity` (GPU-composited, no layout thrash)
2. Use `will-change: transform` on elements that animate frequently (sidebar, side panel)
3. Disable animations when `prefers-reduced-motion: reduce` is set:
   - All transitions become instant (0ms duration)
   - Pulse/breathing animations are disabled
   - Page transitions use cross-fade only (no slide)
   - Skeleton shimmer becomes static gray

### Reduced Motion Media Query
```css
@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after {
    animation-duration: 0.01ms !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0.01ms !important;
  }
}
```
