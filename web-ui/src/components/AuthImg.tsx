// 带鉴权的图片加载。
//
// 为什么需要它：nginx 对 /api/ 做 Basic Auth，而浏览器的 <img src> 请求
// 不经过 auth.ts 里的 window.fetch 拦截器，也就带不上 Authorization 头。
// 以前能显示，是因为浏览器弹过原生登录框、把凭据缓存下来并自动附加到所有
// 同源请求（含图片）。去掉那个弹窗后（见 nginx $http_authorization 守卫），
// <img> 就会 401 变破图 —— 二维码显示不出来、扫不了码。
//
// 做法：用 fetch（走拦截器，自动带 Authorization）拉成 blob，再交给 <img>。
import { useEffect, useState } from "react";

export default function AuthImg({
  src,
  alt,
  className,
  onError,
  onImageClick,
}: {
  src: string;
  alt?: string;
  className?: string;
  onError?: () => void;
  /// 点击回调，回传的是**图片原始尺寸**下的坐标（截图是 1280x800 整页图，
  /// 所以这就是页面坐标，后端可直接拿去 page.mouse.click）。
  onImageClick?: (x: number, y: number) => void;
}) {
  const [objUrl, setObjUrl] = useState<string | null>(null);

  useEffect(() => {
    let dead = false;
    let created: string | null = null;
    void (async () => {
      try {
        const r = await fetch(src);
        if (!r.ok) throw new Error(`HTTP ${r.status}`);
        const blob = await r.blob();
        if (dead) return;
        created = URL.createObjectURL(blob);
        setObjUrl(created);
      } catch {
        if (!dead) {
          setObjUrl(null);
          onError?.();
        }
      }
    })();
    return () => {
      dead = true;
      // 释放上一张，避免长时间轮询刷新累积内存
      if (created) URL.revokeObjectURL(created);
    };
    // onError 每次渲染都是新函数，不进依赖，否则会无限重拉
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [src]);

  if (!objUrl) return null;
  if (!onImageClick) return <img src={objUrl} alt={alt} className={className} />;
  return (
    <img
      src={objUrl}
      alt={alt}
      className={className}
      style={{ cursor: "crosshair" }}
      onClick={(e) => {
        const img = e.currentTarget;
        const r = img.getBoundingClientRect();
        const nw = img.naturalWidth;
        const nh = img.naturalHeight;
        if (!nw || !nh) return;
        // 关键：图片用 object-contain 放在方形容器里，实际画面是「等比缩放 + 居中留白」。
        // 直接用元素外框换算会把留白也算进去 → 坐标整体偏移、点不中。
        // 这里先求真实缩放比和留白偏移，再反推页面坐标。
        const scale = Math.min(r.width / nw, r.height / nh);
        const offX = (r.width - nw * scale) / 2;
        const offY = (r.height - nh * scale) / 2;
        const x = (e.clientX - r.left - offX) / scale;
        const y = (e.clientY - r.top - offY) / scale;
        if (x < 0 || y < 0 || x > nw || y > nh) return; // 点在留白上，忽略
        onImageClick(Math.round(x), Math.round(y));
      }}
    />
  );
}
