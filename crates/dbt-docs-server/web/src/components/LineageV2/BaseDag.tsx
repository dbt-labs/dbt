import { type ReactNode, useCallback, useEffect, useRef, useState } from 'react';
import { useMeasure } from '@uidotdev/usehooks';
import { Background, ConnectionMode, ReactFlow, useReactFlow } from '@xyflow/react';

import { useHydrateLineageStore } from '../../hooks/useHydrateLineageStore';
import { useLayoutWhenMeasured } from '../../hooks/useLayoutWhenMeasured';
import { fqnFromUniqueId } from '../../hooks/useLineageData';
import { useTheme } from '../../hooks/useTheme';
import { Spinner } from '../../shared';
import { UNSUPPORTED_SURFACE_MESSAGE } from '../../shared/hooks/unsupportedSurface';
import {
  useLineageFlow,
  useLineageStatus,
  useLineageStore,
} from '../../stores/lineageStore';
import { DagBottomBar } from './DagBottomBar';
import { DagGroupsView } from './DagGroupsView';
import { DagHopBar } from './DagHopBar';
import { DagMinimapPanel } from './DagMinimapPanel';
import { DAG_NODE_TYPES } from './DagNode';

interface Props {
  rootUniqueId: string;
  /** Rendered on the far left of the hop bar's own holder -- see DagHopBar. */
  topBarLeft?: ReactNode;
  /** Clicking a resource pill in the Groups view bubbles here -- the caller is
   *  expected to make it the new root (BaseDag resets hops to 1+/+1 whenever
   *  rootUniqueId changes, see below). Absent in contexts that don't support
   *  recentering (e.g. the small embedded preview); Groups still renders,
   *  clicking a pill just does nothing. */
  onRecenter?: (uniqueId: string) => void;
  /** Clicking a DAG canvas node bubbles here -- the caller is expected to open
   *  that node's detail drawer. Absent in contexts with no drawer to open
   *  (e.g. the small embedded preview); clicking a node still selects it
   *  (React Flow's own behavior, unchanged), it just doesn't open anything. */
  onNodeClick?: (uniqueId: string) => void;
}

// const LINEAGE_DEPTH = 3;

// Default hop depth, both directions -- matches the Figma states' "1+ / +1" default.
const DEFAULT_HOPS = 1;

// Hoisted: an object literal here would be a new prop identity on every render, which
// React Flow treats as a changed edge default.
const DEFAULT_EDGE_OPTIONS = { type: 'smoothstep' };

// Below this zoom, individual node cards read as unreadable clutter -- collapse
// every unselected node to just its resource badge (see DagNode, isCompact).
const DAG_COMPACT_ZOOM_THRESHOLD = 0.25;

// xyflow's own floor -- the absolute farthest anyone can ever zoom out,
// regardless of graph size. `minZoom` itself is computed per-graph below
// (see the fitView effect) and is usually higher than this; this is just
// the fallback for the first frame, before there's a container size to
// compute the real one from, and the ceiling on how permissive a huge
// graph's computed floor is allowed to be.
const ABSOLUTE_MIN_ZOOM = 0.1;
// How far past a tight `fitView` a user can zoom out before hitting the
// floor -- 0.5 means "half again as much empty space as fitting the whole
// graph exactly." Gates zoom-out to the graph's own size instead of xyflow's
// flat ABSOLUTE_MIN_ZOOM, which on a small graph left a sea of dead canvas
// in every direction.
// const FIT_MIN_ZOOM_RATIO = 0.5;

interface CanvasProps {
  upstreamHops: number;
  downstreamHops: number;
  onUpstreamChange: (hops: number) => void;
  onDownstreamChange: (hops: number) => void;
  topBarLeft?: ReactNode;
  onRecenter?: (uniqueId: string) => void;
  onNodeClick?: (uniqueId: string) => void;
}

function BaseDagCanvas({
  upstreamHops,
  downstreamHops,
  onUpstreamChange,
  onDownstreamChange,
  topBarLeft,
  onRecenter,
  onNodeClick,
}: CanvasProps) {
  const { nodes, edges, onNodesChange, onEdgesChange, onConnect } = useLineageFlow();
  const { status, error, rootUniqueId } = useLineageStatus();
  const [ref, { width, height }] = useMeasure();
  // Positions the cards once React Flow has measured them — see the hook. Must be
  // under the <ReactFlow> below, which is what does the measuring.
  useLayoutWhenMeasured();
  // A boolean rather than the nodes: this component only needs to know whether there
  // is a laid-out graph to frame, and a primitive doesn't change identity on every
  // drag frame.
  const isLaidOut = useLineageStore((s) => s.isLaidOut);
  const { fitView } = useReactFlow();
  const setCompact = useLineageStore((s) => s.setCompact);
  const [view, setView] = useState<'groups' | 'dag'>('dag');
  // A pill click in Groups always drops back to the DAG view, in addition to
  // whatever the caller does with the new root (BaseDag resets hops itself).
  const onSelectGroupResource = useCallback(
    (uniqueId: string) => {
      setView('dag');
      onRecenter?.(uniqueId);
    },
    [onRecenter],
  );
  const fullscreenTarget = useRef<HTMLDivElement>(null);
  // React Flow stamps `light` or `dark` on the canvas root for its own theming, and
  // biga's tokens are scoped by those exact class names (`:root .light` / `:root .dark`
  // in styles/tokens.css). So the canvas is a theme scope whether we want one or not,
  // and leaving it on the default (`light`) re-themes every token inside it — a dark
  // app would render a light node on a dark page. `colorMode="system"` is not an escape
  // hatch either: it resolves to a class from the media query. Feed it the app's
  // resolved theme instead.
  const { resolved } = useTheme();

  // xyflow's own flat 0.1 until the graph-relative floor below computes its
  // first real value.
  const [minZoom, _setMinZoom] = useState(ABSOLUTE_MIN_ZOOM);

  // Frame the graph once it has been laid out. Keyed on `isLaidOut`, not on `nodes` —
  // the latter changes on every drag and would yank the viewport out from under the
  // cursor. Before the layout there is nothing worth framing: the cards are stacked at
  // the origin waiting to be measured, and fitting on that would frame a single point
  // and then have to jump.
  useEffect(() => {
    if (!isLaidOut) return;
    const frame = requestAnimationFrame(() => fitView({ padding: 0.2, duration: 200 }));
    return () => cancelAnimationFrame(frame);
  }, [isLaidOut, rootUniqueId, fitView]);

  if (status === 'error' && error) {
    return (
      <div className="err">
        Failed to load lineage: <code className="inline">{error.message}</code>
      </div>
    );
  }
  if (status === 'unsupported') {
    return (
      <p className="muted" style={{ fontSize: 13 }}>
        {UNSUPPORTED_SURFACE_MESSAGE}
      </p>
    );
  }
  if (nodes.length === 0) {
    return (
      <p className="muted flex items-center gap-2" style={{ fontSize: 13 }}>
        <Spinner /> Loading lineage…
      </p>
    );
  }

  return (
    <div ref={fullscreenTarget} className="dag-v2-canvas relative h-full w-full">
      {view === 'dag' ? (
        <ReactFlow
          nodes={nodes}
          edges={edges}
          onNodesChange={onNodesChange}
          onEdgesChange={onEdgesChange}
          onConnect={onConnect}
          onNodeClick={(_, node) => onNodeClick?.(node.id)}
          // Single subscription point for the LOD compact state -- fires on every
          // pan/zoom frame, but setCompact no-ops unless the threshold is actually
          // crossed, so this doesn't re-render anything on most frames.
          onMove={(_, viewport) =>
            setCompact(viewport.zoom < DAG_COMPACT_ZOOM_THRESHOLD)
          }
          connectionMode={ConnectionMode.Loose}
          fitView
          attributionPosition="bottom-left"
          minZoom={minZoom}
          maxZoom={2}
          defaultEdgeOptions={DEFAULT_EDGE_OPTIONS}
          nodeTypes={DAG_NODE_TYPES}
          colorMode={resolved}
          nodesConnectable={false}
          nodesDraggable={false}
          elementsSelectable={true}
          ref={ref}
        >
          <Background color="var(--muted-foreground)" gap={20} size={1} />
        </ReactFlow>
      ) : (
        rootUniqueId && (
          <DagGroupsView
            rootUniqueId={rootUniqueId}
            onSelectResource={onSelectGroupResource}
          />
        )
      )}
      {/* inset-x-4, mirroring DagBottomBar -- topBarLeft (the caller's close+
       *  breadcrumb, if any) and the hop cluster now share this one holder, so
       *  there's no separate overlay to collide with. */}
      <div className="absolute inset-x-4 top-4 z-10">
        <DagHopBar
          path={rootUniqueId ? fqnFromUniqueId(rootUniqueId) : ''}
          upstreamHops={upstreamHops}
          downstreamHops={downstreamHops}
          onUpstreamChange={onUpstreamChange}
          onDownstreamChange={onDownstreamChange}
          leftContent={topBarLeft}
        />
      </div>
      {/* Both canvas-docked, not children of <ReactFlow> -- sized against the
       *  container so they stay a fixed size regardless of pan/zoom, same trick
       *  xyflow's own <Panel> uses internally. Together these replace xyflow's
       *  default <Controls>/<MiniMap>: DagZoomControl (inside DagBottomBar) covers
       *  zoom, DagMinimapPanel covers the overview thumbnail. */}
      {/* bottom-20: sits just above DagBottomBar (bottom-4 + its own ~56px height +
       *  a gap), so the two don't collide once the minimap is expanded. */}
      {(height ?? 601) > 600 && (width ?? 601) > 600 && (
        <div className="absolute bottom-20 right-4 z-10">
          <DagMinimapPanel view={view} rootUniqueId={rootUniqueId ?? ''} />
        </div>
      )}
      <div className="absolute inset-x-4 bottom-4 z-10">
        <DagBottomBar
          view={view}
          onViewChange={setView}
          onRefresh={() => fitView({ padding: 0.2, duration: 200 })}
          fullscreenTarget={fullscreenTarget}
        />
      </div>
    </div>
  );
}

/** Owns the one fetch: hydrates the app-wide lineage store from DuckDB, then renders
 *  a canvas that — like any other view of the same graph — reads it back out of the
 *  store rather than taking it as props. Also owns the hop-bar's upstream/downstream
 *  state, since both directions ultimately feed the one fetch below. */
export function BaseDag({ rootUniqueId, topBarLeft, onRecenter, onNodeClick }: Props) {
  const [upstreamHops, setUpstreamHops] = useState(DEFAULT_HOPS);
  const [downstreamHops, setDownstreamHops] = useState(DEFAULT_HOPS);
  // Recentering (from a Groups pill, or any other future "make this the root"
  // action) always lands on a fresh 1+/+1 -- without this, hops from the
  // previous root would silently carry over, since BaseDag doesn't remount
  // when only its rootUniqueId prop changes.
  //
  // Adjusted during render, not in an effect. An effect runs after the commit, so
  // the new root would first render -- and fetch -- at the *previous* root's hops,
  // then reset and fetch a second time. Setting state during render instead makes
  // React throw this render away and immediately re-run the body with 1+/+1 already
  // in place, before committing anything or running an effect, so the fetch below
  // only ever sees the reset value. This is React's documented pattern for
  // "adjusting state when a prop changes".
  // https://react.dev/reference/react/useState#storing-information-from-previous-renders
  const [hopsRoot, setHopsRoot] = useState(rootUniqueId);
  if (hopsRoot !== rootUniqueId) {
    setHopsRoot(rootUniqueId);
    setUpstreamHops(DEFAULT_HOPS);
    setDownstreamHops(DEFAULT_HOPS);
  }
  useHydrateLineageStore(rootUniqueId, upstreamHops, downstreamHops);
  return (
    <BaseDagCanvas
      upstreamHops={upstreamHops}
      downstreamHops={downstreamHops}
      onUpstreamChange={setUpstreamHops}
      onDownstreamChange={setDownstreamHops}
      topBarLeft={topBarLeft}
      onRecenter={onRecenter}
      onNodeClick={onNodeClick}
    />
  );
}
