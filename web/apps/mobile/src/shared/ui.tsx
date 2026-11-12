// Small presentational building blocks shared by every screen.
import type { ReactNode } from "react";

export function Screen({ title, onBack, children }: { title: string; onBack?: () => void; children: ReactNode }) {
  return (
    <main className="screen">
      <header className="bar">
        {onBack ? (
          <button className="back" onClick={onBack} aria-label="뒤로">
            ‹
          </button>
        ) : (
          <span />
        )}
        <h1>{title}</h1>
        <span />
      </header>
      <section className="content">{children}</section>
    </main>
  );
}

export function Field(props: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  type?: string;
  inputMode?: "numeric" | "tel" | "text";
  name: string;
}) {
  return (
    <label className="field">
      <span>{props.label}</span>
      <input
        name={props.name}
        type={props.type ?? "text"}
        inputMode={props.inputMode}
        value={props.value}
        onChange={(e) => props.onChange(e.target.value)}
      />
    </label>
  );
}

export function Notice({ kind, children }: { kind: "error" | "ok"; children: ReactNode }) {
  return (
    <p className={`notice ${kind}`} role={kind === "error" ? "alert" : "status"}>
      {children}
    </p>
  );
}
