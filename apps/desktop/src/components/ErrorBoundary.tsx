import { AlertTriangle, RotateCcw } from "lucide-react";
import { Component, type ErrorInfo, type ReactNode } from "react";
import { api } from "../lib/api";

type Props = {
  /** Nome da área, mostrado na mensagem ("o painel Legendas", "a interface"). */
  area: string;
  children: ReactNode;
  /** Ocupa a tela toda (raiz do app) em vez de um bloco no painel. */
  full?: boolean;
  /** Muda quando o conteúdo deve ser recriado (ex.: aba ou projeto trocados). */
  resetKey?: unknown;
};
type State = { error: Error | null; logPath: string | null };

/**
 * Sem isto, uma exceção ao desenhar qualquer componente desmonta a árvore React
 * inteira e o app fica com a tela vazia (preta), sem resposta. Aqui o erro fica
 * contido na área que falhou, é registrado em `logs/ui-errors.log` e o usuário
 * pode tentar de novo sem perder o resto da interface.
 */
export class ErrorBoundary extends Component<Props, State> {
  state: State = { error: null, logPath: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    const stack = `${error.stack ?? ""}\n\nComponentes:${info.componentStack ?? ""}`;
    api
      .reportUiError(`[${this.props.area}] ${error.message}`, stack)
      .then((logPath) => this.setState({ logPath }))
      .catch(() => {});
  }

  componentDidUpdate(prev: Props) {
    if (this.state.error && prev.resetKey !== this.props.resetKey) this.setState({ error: null, logPath: null });
  }

  render() {
    const { error, logPath } = this.state;
    if (!error) return this.props.children;
    return (
      <div role="alert" className={this.props.full ? "flex h-screen items-center justify-center bg-ink-0 p-8" : "p-2"}>
        <div className="max-w-lg rounded-lg border border-bad/40 bg-bad/10 p-3 text-xs text-fg">
          <p className="flex items-center gap-2 font-semibold text-bad">
            <AlertTriangle size={14} /> Ocorreu um erro em {this.props.area}
          </p>
          <p className="mt-1.5 break-words text-fg-2">{error.message}</p>
          {logPath && <p className="mt-1.5 break-all text-[11px] text-fg-3">Detalhes salvos em {logPath}</p>}
          <button
            type="button"
            className="mt-2.5 inline-flex items-center gap-1.5 rounded-md border border-line px-2.5 py-1 hover:bg-ink-3"
            onClick={() => this.setState({ error: null, logPath: null })}
          >
            <RotateCcw size={12} /> Tentar de novo
          </button>
        </div>
      </div>
    );
  }
}
