import type { ChromeDict } from "../types";

/**
 * Vietnamese chrome pack. Native rewrite mirroring the current English copy —
 * the "any model, on your machine" wordmark tag and the deep-dive tagline;
 * no trace of the old positioning. Terminology follows the TUI locale pack
 * (`crates/tui/locales/vi.json`) so the site and the runtime name the same
 * things the same way: nhà cung cấp (provider), phiên (session), kho mã
 * (repository), mức quyền (permission posture), biên nhận (receipt). Product
 * terms — Codewhale, Plan / Work / Operate, Ask / Auto-Review / Full Access,
 * fleet, Workflow, Runtime — stay literal, as they do in the TUI.
 *
 * The masthead pairs a Vietnamese primary label with a short English
 * secondary one; the Han seals are the English edition's own device and are
 * not borrowed here (only the 深 wordmark glyph is shared).
 */
export const chrome: ChromeDict = {
  navDocs: "Tài liệu",
  navStart: "Bắt đầu",
  navInstall: "Cài đặt",
  // "Câu hỏi thường gặp" is the full form; the horizontal masthead nav needs
  // the short one. The footer, which stacks vertically, keeps the full form.
  navFaq: "Hỏi đáp",
  navCommunity: "Cộng đồng",
  navContribute: "Đóng góp",

  navDocsSecondary: "Docs",
  navStartSecondary: "Start",
  navInstallSecondary: "Install",
  navFaqSecondary: "FAQ",
  navCommunitySecondary: "Community",
  navContributeSecondary: "Contribute",

  skipToContent: "Bỏ qua tới nội dung chính",


  navPrimaryAria: "Điều hướng chính",
  navHomeAria: "Trang chủ Codewhale",

  installCta: "Cài đặt →",

  authSignIn: "Đăng nhập",
  authRegister: "Đăng ký",
  authGroupAria: "Tài khoản",

  wordmarkSeal: "深",
  wordmarkTag: "mọi mô hình, trên máy của bạn",

  issueLabel: "Số ra {date}",
  dateLocale: "vi-VN",

  starsAria: "Số sao trên GitHub",
  githubFallback: "GitHub",

  tickerLiveLabel: "Trực tiếp",
  tickerLiveTag: "LIVE",
  tickerMerged: "đã hợp nhất",
  tickerOpened: "đã mở",
  tickerClosed: "đã đóng",
  tickerReleased: "đã phát hành",
  tickerFirstContribution: "đóng góp đầu tiên",
  tickerBy: "bởi {handle}",
  tickerAria: "Hoạt động gần đây của kho mã",

  traceLabel: "mạch suy luận",
  traceTabsAria: "Trích đoạn phiên làm việc",

  menuOpen: "Mở menu",
  menuClose: "Đóng menu",

  themeAuto: "tự động",
  themeLight: "sáng",
  themeDark: "tối",
  themeAria: "Giao diện tài liệu: {mode} (nhấn để đổi)",
  themeTitle: "Giao diện tài liệu · tự động / sáng / tối",

  footerTagline:
    "Codewhale lặn xuống biển sâu để bạn khỏi phải lặn — tài liệu, mã nguồn và cộng đồng của runtime mã nguồn mở.",
  footerProduct: "Sản phẩm",
  footerProject: "Dự án",
  footerDocs: "Tài liệu",
  footerGuide: "Hướng dẫn bắt đầu",
  footerInstall: "Cài đặt",
  footerModels: "Mô hình",
  footerRuntime: "Runtime",
  footerFaq: "Câu hỏi thường gặp",
  // The GitHub tracker, named as GitHub names it — the same choice id/ja/pt-BR made.
  footerIssues: "Issues",
  footerContribute: "Đóng góp",
  footerLicense: "Giấy phép MIT",
  footerPricing: "Bảng giá",
  footerTerms: "Điều khoản dịch vụ",
  footerPrivacy: "Quyền riêng tư",
  footerChangelog: "Nhật ký thay đổi",
  footerCanonicalSource: "Nguồn chính thức: ",
  footerReleases: " · Bản phát hành: ",
  footerReleasesLink: "Bản phát hành trên GitHub",
  footerSecurity: "Bảo mật",

  switcherLabel: "Ngôn ngữ",
  switcherSwitchTo: "Chuyển sang {label}",
  partialBadge: "(một phần)",
};
