import { NavLink, Route, Routes } from "react-router-dom";
import Dashboard from "./pages/Dashboard";
import Accounts from "./pages/Accounts";
import Trade from "./pages/Trade";
import Strategy from "./pages/Strategy";
import Login from "./pages/Login";
import ServerExpiryCard from "./components/ServerExpiryCard";
import { logout } from "./auth";

const navItems = [
  { path: "/", label: "Dashboard" },
  { path: "/accounts", label: "账户" },
  { path: "/trade", label: "交易" },
  { path: "/strategy", label: "策略" },
];

function navClass(isActive: boolean): string {
  return `px-3 py-2 rounded text-sm whitespace-nowrap ${
    isActive
      ? "bg-neutral-800 text-white"
      : "text-neutral-400 hover:bg-neutral-800/50 hover:text-neutral-200"
  }`;
}

function Shell() {
  return (
    <div className="min-h-screen md:flex">
      {/* 桌面：左侧栏 */}
      <aside className="hidden md:flex w-48 shrink-0 bg-neutral-900 border-r border-neutral-800 p-4 flex-col">
        <div className="text-lg font-semibold mb-6">new-alpha-trade</div>
        <nav className="flex flex-col gap-1">
          {navItems.map((item) => (
            <NavLink key={item.path} to={item.path} end className={({ isActive }) => navClass(isActive)}>
              {item.label}
            </NavLink>
          ))}
        </nav>
        <div className="mt-auto space-y-2">
          <ServerExpiryCard />
          <button
            onClick={logout}
            className="w-full text-xs text-neutral-500 hover:text-neutral-300 border border-neutral-800 rounded px-2 py-1.5"
          >
            退出登录
          </button>
        </div>
      </aside>

      {/* 手机：顶部横向导航（可横滑） */}
      <header className="md:hidden sticky top-0 z-40 bg-neutral-900/95 backdrop-blur border-b border-neutral-800">
        <div className="flex items-center gap-2 px-3 py-2 overflow-x-auto">
          <span className="text-sm font-semibold whitespace-nowrap mr-1">new-alpha-trade</span>
          <nav className="flex gap-1">
            {navItems.map((item) => (
              <NavLink key={item.path} to={item.path} end className={({ isActive }) => navClass(isActive)}>
                {item.label}
              </NavLink>
            ))}
          </nav>
          <button
            onClick={logout}
            className="ml-auto text-xs text-neutral-500 hover:text-neutral-300 whitespace-nowrap px-2 py-1"
          >
            退出
          </button>
        </div>
      </header>

      <main className="flex-1 min-w-0 p-4 md:p-6">
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

export default function App() {
  return (
    <Routes>
      {/* 登录整页（无侧栏/顶栏，专注扫码） */}
      <Route path="/login" element={<Login />} />
      <Route path="/*" element={<Shell />} />
    </Routes>
  );
}
