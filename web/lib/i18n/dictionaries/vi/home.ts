import type { HomeDict } from "../types";

/**
 * Vietnamese home pack. Native rewrite mirroring the current English copy —
 * the "dives into the deep / any model, on your machine" direction; no trace
 * of the old positioning. Terminology matches `vi/chrome.ts` and the TUI
 * locale pack: nhà cung cấp (provider), phiên (session), kho mã
 * (repository), mức quyền (permission posture), biên nhận (receipt), nhiệm
 * vụ (task). Modes (Plan / Work / Operate), permission postures (Ask /
 * Auto-Review / Full Access), commands (`codewhale exec`), fleet, Workflow,
 * Runtime and the product name stay literal, exactly as the TUI renders
 * them.
 *
 * The `seal*` glyphs are marks, not prose, and are shared with English.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — lặn xuống biển sâu để bạn khỏi phải lặn.",
  metaDescription:
    "Codewhale lặn xuống biển sâu để bạn khỏi phải lặn — tác nhân lập trình mã nguồn mở trong terminal. Mang theo mô hình của bạn. Chạy trên máy của bạn. Rust, MIT.",

  kicker: "Nguồn mở · Mang theo mô hình của bạn · Chạy trong terminal của bạn",
  heroTitleA: "Codewhale lặn xuống biển sâu",
  heroTitleB: "để bạn khỏi phải lặn.",
  heroIntro:
    "{brand} là tác nhân lập trình mã nguồn mở trong terminal của bạn. Giao cho nó một mô hình và một nhiệm vụ — nó đọc mã của bạn, sửa tệp, tự chạy kiểm tra, và dừng lại khi việc đã xong hoặc khi nó cần bạn. Mang bất kỳ mô hình nào, hoặc trộn nhiều mô hình: ghim một mô hình riêng cho từng vai trò.",
  install: "Cài đặt",
  docs: "Tài liệu",
  copy: "Sao chép",
  copied: "Đã sao chép ✓",

  installEyebrow: "cài đặt một dòng lệnh",
  installRequirement: "cần Node 18+ — không cần bộ công cụ Rust",
  installOtherWays: "cách khác →",

  latestRelease: "Bản phát hành mới nhất {tag}",
  releaseUnavailable: "Không có trạng thái phát hành",
  currentSource: "Mã nguồn",
  sourceCandidate: "Chưa phát hành",
  providerRoutes: "{count} nhà cung cấp",
  publishedRelease: "đã phát hành",
  figcaptionSourceCandidate: "chưa phát hành",

  shotSession: "Phiên hiện tại",
  screenshotAlt:
    "Phiên terminal Codewhale hiện tại hiển thị chế độ Operate, hình cá voi, khung soạn thảo và thanh chân màn hình",
  figcaption: "Phiên Codewhale hiện tại · chế độ Operate · mức quyền Ask",

  proofHeading: "Một lớp vỏ terminal dưới lòng biển. Mọi mô hình. Trên máy của bạn.",
  proofBody:
    "Mang theo mô hình bạn đang dùng — hosted, gateway hoặc cục bộ. Plan / Work / Operate cùng các mức quyền khai báo rõ giữ cuộc lặn luôn trong tầm kiểm soát của bạn.",

  sealDecides: "法",
  decidesEyebrow: "Xem cách nó quyết định",
  decidesHeading: "Luật lệ bạn quan sát được ngay trong mạch suy luận",
  decidesLede:
    "Trích đoạn từ phiên thật — thứ bậc luật lệ của dự án thấy được trong suy luận của mô hình, không chỉ là lời hứa trên trang chủ.",

  sealWorkflow: "行",
  workflowHeading: "Từ nhiệm vụ đến thay đổi đã kiểm chứng.",
  workflow: [
    ["Khảo sát", "Đọc kho mã, các hướng dẫn của nó và nhiệm vụ."],
    ["Hành động", "Sửa tệp trong ranh giới phê duyệt rõ ràng."],
    ["Xác minh", "Chạy các bước kiểm tra và xem kết quả."],
    ["Báo cáo", "Để lại một biên nhận ngắn gọn, bền lâu."],
  ],
  receiptAria: "Ví dụ biên nhận công việc",
  receiptInspect: "kho mã và hướng dẫn",
  receiptAct: "sửa theo mức quyền đã chọn",
  receiptReport: "kiểm tra đạt · đã lưu biên nhận",

  sealStart: "起",
  startHeading: "Mới dùng Codewhale? Bốn bước từ đầu đến cuối.",
  startLede:
    "Cài đặt → phiên đầu không cần khóa → kết nối nhà cung cấp → Workflow fleet đầu tiên. Thuật ngữ được định nghĩa ở trang thuật ngữ.",
  startGuideLink: "Đọc hướng dẫn bắt đầu →",
  startVocabularyLink: "Xem thuật ngữ sản phẩm →",

  sealBoundaries: "界",
  boundariesHeadingA: "Mô hình của bạn.",
  boundariesHeadingB: "Ranh giới của bạn.",
  boundariesBody:
    "Chọn rõ ràng mô hình, chế độ làm việc và mức quyền. Chi phí chưa biết vẫn được ghi là chưa biết, và những phần còn ở bản xem trước luôn được ghi nhãn đúng như vậy.",
  hostedGatewayLocal: "Mô hình hosted, gateway và cục bộ",
  planActOperateDesc: "Từ lập kế hoạch chỉ đọc đến vận hành tự chủ",
  askAutoReviewDesc: "Chọn mức quyền cho công việc",
  tuiExecWebDesc: "Giao diện runtime tương tác và headless",

  sealSurfaces: "面",
  surfacesHeading: "Dùng runtime ngay nơi công việc diễn ra.",
  surfaces: [
    ["TUI", "Làm việc tương tác trong terminal"],
    ["codewhale exec", "Script và CI"],
    ["Ứng dụng web", "Chạy trong trình duyệt, chỉ qua loopback"],
    ["Runtime API + MCP", "Tích hợp cục bộ"],
    ["fleet", "Công việc nhiều tác tử, bền vững"],
  ],
  runtimeLink: "Xem các giao diện runtime và ghi chú về độ ổn định →",

  installBandHeading: "Bắt đầu chỉ bằng một lệnh.",
  binaries: "Bản nhị phân",
  chinaMirrors: "Mirror Trung Quốc",
  installGuideLink: "Đọc hướng dẫn cài đặt →",

  sealCommunity: "众",
  communityHeading: "Xây dựng công khai",
  communityBody:
    "Giấy phép MIT, được định hình bởi những người đóng góp trên khắp runtime, nhà cung cấp, nền tảng, tài liệu và kiểm thử.",
  communityLinksAria: "Liên kết cộng đồng",
  contribute: "Đóng góp",
};
