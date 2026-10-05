import { Component, type ErrorInfo, type ReactNode } from "react";

type Props = { children: ReactNode };
type State = { error: Error | null };

/**
 * Last-resort boundary around the whole dashboard. The prediction engine runs
 * during render (via useMemo), so an unexpected math edge case would otherwise
 * unmount the entire tree and leave the tray window silently black. This does
 * not replace fixing the underlying defect: the error is logged, and the
 * fallback names it and offers a reload instead of swallowing it.
 */
export class AppErrorBoundary extends Component<Props, State> {
  state: State = { error: null };

  static getDerivedStateFromError(error: Error): State {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo): void {
    console.error("LimitScope failed to render:", error, info.componentStack);
  }

  render() {
    const { error } = this.state;
    if (error === null) return this.props.children;
    return (
      <div className="crash-fallback" role="alert">
        <h1 className="title">LimitScope</h1>
        <p className="crash-message">
          The dashboard hit an unexpected error and stopped rendering.
        </p>
        <p className="crash-detail">{error.message}</p>
        <button
          type="button"
          className="clear-history-btn"
          onClick={() => window.location.reload()}
        >
          Reload
        </button>
      </div>
    );
  }
}
