// What the console's page receives: the shapes of /api/topology and
// /api/state (from tinytrader-fleet), and the widget contract (decision
// 0010). Checked against the page's JavaScript by `tsc -p services/console`.

/** A schedule entry a service describes (`/describe` → `schedule`). */
interface ScheduleEntry {
  name: string;
  kind: 'window' | 'pause' | 'yield' | 'always' | 'interval' | 'event';
  when?: string;
  /** `HH:MM-HH:MM`, New York time, set relative to the regular session. */
  hours?: string;
  every_seconds?: number;
  feed?: string;
  symbols?: string[];
  class?: string;
  yields_to?: string[];
}

interface TopoNode {
  id: string;
  label?: string;
  sub?: string;
  role?: string;
  host?: string;
}

interface TopoService extends TopoNode {
  schedule?: ScheduleEntry[];
  /** One symbol's one-minute bars, on demand (`?symbol=SPY&days=2`). */
  bars?: string;
  events?: string;
  accounts?: unknown;
  observe?: string;
}

interface TopoEdge {
  from: string;
  to: string;
  kind: 'flow' | 'read' | string;
  label?: string;
}

/** A bundle (decision 0011): services sharing hours, with a work class. */
interface Bundle {
  name: string;
  class: 'capture' | 'interactive' | 'background';
  members: string[];
  note?: string;
}

/** Price cards a topology declares: one per key of a service's series stream. */
interface Panel {
  service: string;
  source?: string;
  heading?: string;
  note?: string;
  title?: string;
  decimals?: number;
  prefix?: string;
  suffix?: string;
  format?: string;
  footer?: { metric: string; match?: string; format?: string; decimals?: number }[];
}

/** A brokerage account as the accounts service serves it. */
interface Account {
  label?: string;
  environment?: 'live' | 'paper' | string;
  updated_at?: number;
  account?: Record<string, any>;
  positions?: Record<string, any>[];
}

interface Topology {
  name?: string;
  focus?: string;
  hosts?: { id: string; label?: string; sub?: string }[];
  services: TopoService[];
  external?: TopoNode[];
  stores?: (TopoNode & { metric?: { service: string; name: string; label?: string } })[];
  edges?: TopoEdge[];
  panels?: Panel[];
  bundles?: Bundle[];
}

/** A part of the deployment graph: an outside source, a service or a store. */
type GraphNode = TopoService & {
  kind: 'ext' | 'svc' | 'store';
  metric?: { service: string; name: string; label?: string; format?: string; decimals?: number };
};

/** A service's health (`/health`), as the fleet polled it. */
interface Health {
  state: 'starting' | 'ready' | 'degraded' | 'idle' | string;
  detail?: string;
  build?: string;
  uptime_seconds?: number;
  last_error?: string;
  last_error_at?: number;
}

interface Metric {
  labels: Record<string, string>;
  value: number;
}

interface Container {
  state: string;
  status: string;
  image?: string;
  started_at?: string;
  restart_count?: number;
  cpu_percent?: number;
  memory?: { usage_bytes?: number };
}

/** A schema's files under one dataset on one drive. */
interface SchemaFiles {
  bytes: number;
  /** Bytes per day, by `YYYY-MM-DD`. */
  days: Record<string, number>;
  encodings?: string[];
  files?: number;
}

interface CatalogDataset {
  id: string;
  symbol?: string;
  sources?: string[];
  schemas: Record<string, SchemaFiles>;
}

interface Volume {
  total_bytes?: number;
  available_bytes?: number;
  used_fraction?: number;
  status: 'ok' | 'warning' | 'critical' | 'floor' | 'unknown';
  reason?: string;
}

interface CatalogRoot {
  name: string;
  path: string;
  present: boolean;
  writable?: boolean;
  volume?: Volume;
  datasets?: CatalogDataset[];
  dataset_bytes?: number;
  other?: { name: string; bytes: number; files?: number; modified?: number }[];
  other_bytes?: number;
  unrecognised?: string[];
  thresholds?: Thresholds;
  service?: string;
}

interface Thresholds {
  warning: number;
  critical: number;
  floor: number;
}

/** A drive the store found but doesn't use (decision 0007). */
interface Candidate {
  path: string;
  label: string;
  total_bytes?: number;
  available_bytes?: number;
}

interface Catalog {
  generated_at: number | null;
  thresholds?: Thresholds;
  roots: CatalogRoot[];
  candidates?: Candidate[];
}

/** Everything the fleet knows about one service. */
interface ServiceState {
  health?: { body?: Health; error?: string };
  container?: Container;
  metrics?: Record<string, Metric[]>;
  catalog?: Catalog;
  calendar?: { clock?: { is_open: boolean; next_open?: string; next_close?: string }; days: { date: string; open: string; close: string }[] };
  econ?: { events: { at: number; date: string; time: string; title: string; source: string; importance: 'low' | 'medium' | 'high' }[]; sources?: Record<string, { events: number; error: string | null }> };
  news?: { items: { id: string; headline: string; summary?: string; url?: string; source?: string; author?: string; symbols?: string[]; created_at?: string }[] };
  accounts?: { accounts: Account[] };
  /** Series streams (such as `recent`): key → [[unix_ms, value], ...]. */
  [stream: string]: unknown;
}

interface Snapshot {
  services: Record<string, ServiceState>;
  docker?: { configured: boolean; error?: string };
  error?: string;
}

/** TradingView's Lightweight Charts (standalone build), loaded by the Candles widget. */
declare var LightweightCharts: any;

/** A mounted widget (decision 0010). */
interface WidgetInstance {
  /** After each poll of the fleet. */
  update?(): void;
  /** When its panel closes or the workspace changes. */
  dispose?(): void;
}

interface WidgetDef {
  title: string;
  mount(el: HTMLElement): WidgetInstance;
}
