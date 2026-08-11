import * as ipc from "@/lib/ipc";
import { Panel, Row } from "./kit";

const DISCORD = "https://discord.gg/rUk4zCfMc8";

const SP_CYCLE_TEAM = ["DADDY DRAVEN", "Slick Daddy"];

const RECYCLE_TEAM = [
  "DisguisedCoBot",
  "laubi",
  "Sable Ward",
  "Sewazur",
  "Sol-Low",
  "Stealth_Master",
];

function People({ title, names }: { title: string; names: string[] }) {
  return (
    <Panel title={`${title} — ${names.length}`}>
      <ul className="flex flex-wrap gap-x-6 gap-y-1.5">
        {names.map((n) => (
          <li key={n} className="text-ink-2">
            {n}
          </li>
        ))}
      </ul>
    </Panel>
  );
}

export function AboutTab({ version }: { version: string }) {
  // through the backend: the webview has no opener of its own, and open_link
  // refuses anything that is not http(s)
  const open = () => {
    // nothing to fall back to if the host has no browser registered, but the
    // reason belongs somewhere other than nowhere
    void ipc.openLink(DISCORD).catch((e) => console.error("could not open Discord:", e));
  };

  return (
    <div className="space-y-6">
      <Panel title="SPCycle">
        <Row label="Launcher" value="Made by Slick Daddy" mono={false} />
        <Row label="Version" value={version} />
        <Row label="Game" value="The Cycle: Frontier — singleplayer" mono={false} />
      </Panel>

      <People title="SP-Cycle Team" names={SP_CYCLE_TEAM} />
      <People title="Project: ReCycle Team" names={RECYCLE_TEAM} />

      <Panel title="Community">
        <div className="py-1.5">
          <p className="text-ink-2">
            Questions, bug reports and everything else happen on Discord.
          </p>
          {/* selectable as well as clickable: on a machine with no browser
              registered, copying the link is the only way through */}
          <button
            onClick={open}
            className="mt-2 text-amber underline underline-offset-2 transition-colors select-text hover:text-ink"
          >
            {DISCORD}
          </button>
        </div>
      </Panel>

      <Panel title="Legal">
        <p className="py-1.5 text-ink-3">
          SPCycle is an unofficial, fan-made project. It is not affiliated with,
          endorsed by, or associated with YAGER Development GmbH, and carries no
          endorsement from any of the studios or publishers behind The Cycle:
          Frontier. All trademarks and game content are the property of their
          respective owners.
        </p>
      </Panel>
    </div>
  );
}
