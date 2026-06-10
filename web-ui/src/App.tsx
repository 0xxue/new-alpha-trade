import { NavLink, Route, Routes } from "react-router-dom";
import Dashboard from "./pages/Dashboard";
import Accounts from "./pages/Accounts";
import Trade from "./pages/Trade";
import Strategy from "./pages/Strategy";
import ServerExpiryCard from "./components/ServerExpiryCard";

const navItems = [
  { path: "/", label: "Dashboard" },
  { path: "/accounts", label: "账户" },
  { path: "/trade", label: "交易" },
  { path: "/strategy", label: "策略" },
];

export default function App() {
  return (
    <div className="min-h-screen flex">
      <aside className="w-48 bg-neutral-900 border-r border-neutral-800 p-4 flex flex-col">
        <div className="text-lg font-semibold mb-6">new-alpha-trade</div>
        <nav className="flex flex-col gap-1">
          {navItems.map((item) => (
            <NavLink
              key={item.path}
              to={item.path}
              end
              className={({ isActive }) =>
                `px-3 py-2 rounded text-sm ${
                  isActive
                    ? "bg-neutral-800 text-white"
                    : "text-neutral-400 hover:bg-neutral-800/50 hover:text-neutral-200"
                }`
              }
            >
              {item.label}
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto">
          <ServerExpiryCard />
        </div>
      </aside>
      <main className="flex-1 p-6">
        <Routes>
          <Route path="/" element={<Dashboard />} />
          <Route path="/accounts" element={<Accounts />} />
          <Route path="/trade" element={<Trade />} />
          <Route path="/strategy" element={<Strategy />} />
        </Routes>
      </main>
    </div>
  );
}
