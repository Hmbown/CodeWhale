# 24 个需要点 Resolved 的 inline review 评论链接

PR: https://github.com/Hmbown/CodeWhale/pull/2753

| # | 评论摘要 | 文件 | 优先级 | 链接 |
|---|---------|------|--------|------|
| 1 | Critical logic bug: `take_pending_for_tab` removes the task from `pending_tasks` | `tab/delegator.rs` | CRITICAL | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362756) |
| 2 | Potential panic due to subtraction overflow on `area.height - 1` when terminal h | `views/tab_picker.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362762) |
| 3 | Potential panic due to subtraction overflow on `area.width - 5` when terminal wi | `views/tab_picker.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362776) |
| 4 | Update `fail_task` to remove the task from `pending_tasks` using `swap_remove` o | `tab/delegator.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362780) |
| 5 | Update `cancel_task` to remove the task from `pending_tasks` using `swap_remove` | `tab/delegator.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362787) |
| 6 | Expose `pending_tasks` as `pub(crate)` so that `TabManager` can restore delegati | `tab/delegator.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362796) |
| 7 | Correctness bug: `restore_from_snapshot` completely ignores `state.delegations`, | `tab/manager.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362802) |
| 8 | Potential panic due to subtraction overflow on `area.height - 1` when terminal h | `views/tab_switcher.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362804) |
| 9 | Potential panic due to subtraction overflow on `area.height - 2` when terminal h | `views/tab_switcher.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362813) |
| 10 | Update `complete` to remove the task from `pending_tasks` using `swap_remove` on | `tab/delegator.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362819) |
| 11 | Potential panic due to subtraction overflow on `area.height - 4` when terminal h | `views/tab_picker.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362828) |
| 12 | Potential panic due to subtraction overflow on `area.height - 1` when terminal h | `views/tab_picker.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362831) |
| 13 | Potential panic due to direct indexing of `tab_ids[0]` without checking if the v | `tab/cross_tab.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362839) |
| 14 | Potential panic due to subtraction overflow on `area.width - 2` and `area.height | `views/tab_switcher.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362847) |
| 15 | Potential panic due to subtraction overflow on `area.height - 1` when terminal h | `views/tab_switcher.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362854) |
| 16 | Potential panic due to subtraction overflow on `area.width - 5` when terminal wi | `views/tab_switcher.rs` | high | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362865) |
| 17 | Redundant `(area.width as usize).try_into().unwrap_or(u16::MAX)` conversion. Sin | `tab/tab_bar.rs` | medium | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362871) |
| 18 | Simplify `take_next_delegation` since `take_pending_for_tab` now automatically m | `tab/manager.rs` | medium | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362877) |
| 19 | The delegation description is currently hardcoded to `"Task from tab"`. Consider | `ui.rs` | medium | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358362882) |
| 20 | <a href="#"><img alt="P1" src="https://greptile-static-assets.s3.amazonaws.com/b | `tab/manager.rs` | P1 | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358375205) |
| 21 | <a href="#"><img alt="P1" src="https://greptile-static-assets.s3.amazonaws.com/b | `tab/delegator.rs` | P1 | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358375311) |
| 22 | <a href="#"><img alt="P1" src="https://greptile-static-assets.s3.amazonaws.com/b | `views/tab_switcher.rs` | P1 | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358375372) |
| 23 | <a href="#"><img alt="P2" src="https://greptile-static-assets.s3.amazonaws.com/b | `tab/delegator.rs` | P2 | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358375459) |
| 24 | <a href="#"><img alt="P2" src="https://greptile-static-assets.s3.amazonaws.com/b | `views/meeting_view.rs` | P2 | [打开](https://github.com/Hmbown/CodeWhale/pull/2753#discussion_r3358375515) |
