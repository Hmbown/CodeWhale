import type { HomeDict } from "../types";

/**
 * Vietnamese home pack. Terminology matches `vi/chrome.ts` and the TUI
 * locale pack: nhà cung cấp (provider), phiên (session), kho mã
 * (repository), quyền (permissions), biên nhận (receipt), nhiệm vụ (task).
 * Modes (Plan / Act / Operate), permission postures (Ask / Auto-Review /
 * Full Access), commands (`codewhale exec`), Fleet, Workflow, Runtime and
 * the product name stay literal, exactly as the TUI renders them.
 *
 * The `seal*` glyphs are marks, not prose, and are shared with English.
 *
 * Machine-quality translation awaiting native-speaker review.
 */
export const home: HomeDict = {
  metaTitle: "Codewhale — Lặn xuống biển sâu, để bạn khỏi phải lặn.",
  metaDescription:
    "Codewhale lặn xuống biển sâu để bạn khỏi phải lặn — một tác nhân chạy trong terminal, trao cho người bình thường đòn bẩy của LLM để tự làm ra thứ mình cần. Chạy trên máy của bạn. Viết bằng Rust, giấy phép MIT.",

  kicker: "Nguồn mở · Mọi mô hình · Chạy trong terminal của bạn",
  heroTitleA: "Lặn xuống biển sâu,",
  heroTitleB: "để bạn khỏi phải lặn.",
  heroIntro:
    "{brand} trao cho người bình thường đòn bẩy của LLM để tự làm ra thứ mình cần. Ngay trong terminal, nó đọc kho mã, sửa tệp, chạy kiểm tra và để lại biên nhận — mà không đòi hỏi bạn phải biết code từ trước. Nó chạy trên máy của bạn; mô hình chỉ là một thành phần bạn chọn được, không phải sản phẩm.",
  install: "Cài đặt",
  docs: "Tài liệu",
  copy: "Sao chép",
  copied: "Đã sao chép ✓",

  installEyebrow: "cài đặt một dòng lệnh",
  installRequirement: "cần Node 18+ — không cần bộ công cụ Rust",
  installOtherWays: "cách khác →",

  latestRelease: "Bản phát hành mới nhất {tag}",
  releaseUnavailable: "Không có trạng thái phát hành",
  currentSource: "Mã nguồn hiện tại",
  sourceCandidate: "Ứng viên từ mã nguồn",
  providerRoutes: "{count} tuyến nhà cung cấp",
  publishedRelease: "bản phát hành chính thức",
  figcaptionSourceCandidate: "ứng viên từ mã nguồn",

  shotSession: "Phiên hiện tại",
  screenshotAlt:
    "Phiên terminal Codewhale hiện tại hiển thị chế độ Operate, hình cá voi, khung soạn thảo và thanh chân màn hình",
  figcaption: "Phiên Codewhale hiện tại · chế độ Operate · quyền Ask",

  proofHeading: "Một lớp vỏ terminal dưới lòng biển. Trung lập về mô hình. Ưu tiên cục bộ.",
  proofBody:
    "Mang theo mô hình hosted, gateway hoặc cục bộ mà bạn đang dùng. Codewhale chạy trên máy của bạn và xem mô hình là một thành phần bạn chọn được — không phải sản phẩm. Plan / Act / Operate cùng quyền hạn khai báo rõ ràng giữ cho cuộc lặn sâu nằm trong tầm kiểm soát của bạn.",

  sealDecides: "法",
  decidesEyebrow: "Xem cách nó quyết định",
  decidesHeading: "Luật lệ bạn quan sát được ngay trong mạch suy luận",
  decidesLede:
    "Những trích đoạn trung thực từ một phiên làm việc thật — thứ bậc luật lệ của dự án quan sát được trong suy luận của mô hình, chứ không phải một lời quảng cáo trên trang chủ.",

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
    "Cài đặt → phiên đầu tiên không cần khóa → kết nối nhà cung cấp → Workflow Fleet đầu tiên. Các thuật ngữ được định nghĩa ở trang thuật ngữ.",
  startGuideLink: "Đọc hướng dẫn bắt đầu →",
  startVocabularyLink: "Xem thuật ngữ sản phẩm →",

  sealBoundaries: "界",
  boundariesHeadingA: "Mô hình của bạn.",
  boundariesHeadingB: "Ranh giới của bạn.",
  boundariesBody:
    "Chọn rõ ràng mô hình, chế độ làm việc và quyền hạn. Chi phí chưa biết vẫn được ghi là chưa biết, và những phần còn ở bản xem trước luôn được ghi nhãn đúng như vậy.",
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
    ["Fleet", "Công việc nhiều tác tử, bền vững"],
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
