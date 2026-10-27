// 首页的标题栏（图标、程序名、账户、菜单、最小化 / 关闭）。从 dd_home_page.dart 拆出来的，私有类照常互相可见
part of 'dd_home_page.dart';

/// 标题栏：左边图标和程序名，右边账户、菜单、最小化 / 关闭（照 UU，没有最大化按钮，双击标题栏照样最大化）。
/// 主窗口是无边框的，原来的标题栏是首页标签栏画的（那一页现在不显示）；按钮沿用 RustDesk 主窗口那组（关闭时藏到托盘等处理都在里面）
class _TitleBar extends StatelessWidget {
  final VoidCallback onAbout;
  const _TitleBar({required this.onAbout});

  @override
  Widget build(BuildContext context) {
    return SizedBox(
      height: DdSize.titlebarMain,
      child: Row(children: [
        Expanded(
          // 撑满标题栏的高、拖窗口、双击最大化（和远程窗口、子窗口的标题栏同一个）
          child: DdTitleDragArea(
            isMainWindow: true,
            child: Row(children: [
                const SizedBox(width: 16),
                loadIcon(22),
                const SizedBox(width: 8),
                const Text('DreamDesk', style: TextStyle(fontSize: 15, fontWeight: FontWeight.w600, color: DdColors.ink)),
              ]),
          ),
        ),
        const _AccountButton(),
        _MoreButton(onAbout: onAbout),
        const DdWindowButtons(isMainWindow: true, showMaximize: false, height: DdSize.titlebarMain),
      ]),
    );
  }
}

/// 标题栏右边的头像、更多（原型 dd-iconbtn，10-27 用户：只有窗口按钮占满标题栏高）：40 宽的位子里一块 32×32、
/// 6 圆角的悬停底。底色是 PopupMenuButton 自带的 InkWell 按主题 hoverColor 画的（传 child 才是长方形），
/// 它画在最近的 Material 上，所以里面垫一层透明 Material、外面裁圆角
Widget _titleSlot(Widget menuButton) => SizedBox(
      width: 40,
      height: DdSize.titlebarMain,
      child: Center(
        child: ClipRRect(
          borderRadius: BorderRadius.circular(DdRadius.md),
          child: Material(type: MaterialType.transparency, child: menuButton),
        ),
      ),
    );
Widget _titleButton(Widget icon) => SizedBox(width: 32, height: 32, child: Center(child: icon));

/// 标题栏的「更多」（照 UU 的白色圆角菜单）：关于、检查更新。
/// 还没有升级分发（K3 / S3），RustDesk 自带的检查更新只问官方 GitHub、定制客户端不用，所以「检查更新」先如实告诉用户
/// 当前版本、新版本由 IT 安装；有了升级分发再接真的检查
class _MoreButton extends StatelessWidget {
  final VoidCallback onAbout;
  const _MoreButton({required this.onAbout});

  @override
  Widget build(BuildContext context) {
    PopupMenuItem<int> item(int value, DdIcon icon, String label) => PopupMenuItem<int>(
          value: value,
          height: 40,
          child: Row(children: [
            ddIcon(icon),
            const SizedBox(width: 12),
            Text(label, style: const TextStyle(fontSize: 14, color: DdColors.ink)),
          ]),
        );
    return _titleSlot(PopupMenuButton<int>(
      tooltip: translate('More'),
      // 按钮顶在标题栏 8 像素处，菜单出在标题栏下面 46
      offset: const Offset(0, 38),
      child: _titleButton(ddIcon(DdIcon.menu, size: 20)),
      itemBuilder: (_) => [
        item(0, DdIcon.info, translate('About')),
        const PopupMenuDivider(height: 9),
        item(1, DdIcon.arrowUpCircle, translate('Check for updates')),
      ],
      onSelected: (v) => v == 0 ? onAbout() : _checkUpdate(context),
    ));
  }

  Future<void> _checkUpdate(BuildContext context) async {
    final version = await bind.mainGetVersion();
    if (!context.mounted) return;
    showDialog(
      context: context,
      // 底、圆角、标题字号都是主题的 dialogTheme
      builder: (ctx) => AlertDialog(
        title: Text(translate('Check for updates')),
        content: Text('${translate('Version')} $version\n\n${translate('Online updates are not available yet. New versions are installed by IT.')}',
            style: const TextStyle(fontSize: 14, color: DdColors.ink2, height: 1.6)),
        actions: [ElevatedButton(onPressed: () => Navigator.of(ctx).pop(), style: ddCompactButton, child: Text(translate('OK')))],
      ),
    );
  }
}

/// 标题栏的头像：点开是一张小卡片（照原型 HomeMenus）——没登录：灰头像、一句提示、整宽「立即登录」；
/// 登录了：头像（名字首字）和显示名、账号横排，下面一行「退出登录」
class _AccountButton extends StatelessWidget {
  const _AccountButton();

  @override
  Widget build(BuildContext context) {
    return _titleSlot(PopupMenuButton<int>(
      tooltip: '',
      offset: const Offset(0, 38),
      // 登录了是蓝底圆形、写名字首字（照原型），没登录是人像
      child: _titleButton(Obx(() {
        final name = gFFI.userModel.displayNameOrUserName;
        if (gFFI.userModel.userName.value.isEmpty || name.isEmpty) return ddIcon(DdIcon.user, size: 20);
        return DdAvatar(name: name, size: 24, fontSize: 12);
      })),
      itemBuilder: (ctx) => [
        _CardEntry(child: Obx(() {
          final account = gFFI.userModel.userName.value;
          final name = gFFI.userModel.displayNameOrUserName;
          final signedIn = account.isNotEmpty;
          // 登录了（照原型）：左边头像、右边名字和账号，下面一行「退出登录」
          if (signedIn) {
            return SizedBox(
              width: 260,
              child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                Padding(
                  padding: const EdgeInsets.fromLTRB(18, 16, 18, 14),
                  child: Row(children: [
                    DdAvatar(name: name, size: 44, fontSize: 18),
                    const SizedBox(width: 12),
                    Expanded(
                      child: Column(crossAxisAlignment: CrossAxisAlignment.start, children: [
                        Text(name,
                            overflow: TextOverflow.ellipsis,
                            style: const TextStyle(fontSize: 14, fontWeight: FontWeight.w600, color: DdColors.ink)),
                        if (name != account) ...[
                          const SizedBox(height: 2),
                          Text(account,
                              overflow: TextOverflow.ellipsis, style: const TextStyle(fontSize: 12, color: DdColors.ink2)),
                        ],
                      ]),
                    ),
                  ]),
                ),
                const Divider(height: 1, color: DdColors.line),
                InkWell(
                  onTap: () {
                    Navigator.of(ctx).pop();
                    gFFI.userModel.logOut();
                  },
                  child: Padding(
                    padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 12),
                    child: Row(children: [
                      ddIcon(DdIcon.logOut),
                      const SizedBox(width: 10),
                      Text(translate('Sign out'), style: const TextStyle(fontSize: 14, color: DdColors.ink)),
                    ]),
                  ),
                ),
              ]),
            );
          }
          return SizedBox(
            width: 220,
            child: Padding(
              padding: const EdgeInsets.fromLTRB(16, 18, 16, 16),
              child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
                Center(
                  child: Container(
                    width: 52,
                    height: 52,
                    alignment: Alignment.center,
                    // 没登录的灰头像：只此一处用的两个灰，不收进 DdColors
                    decoration: const BoxDecoration(color: Color(0xFFE4E6EA), shape: BoxShape.circle),
                    child: ddIcon(DdIcon.user, size: 30, color: const Color(0xFFB8BDC5)),
                  ),
                ),
                const SizedBox(height: 12),
                Text(translate('Sign in to see the workstations assigned to you.'),
                    textAlign: TextAlign.center, style: const TextStyle(fontSize: 14, color: DdColors.ink, height: 1.5)),
                const SizedBox(height: 16),
                ElevatedButton(
                  onPressed: () {
                    Navigator.of(ctx).pop();
                    _showLogin(context);
                  },
                  child: Text(translate('Sign in now')),
                ),
              ]),
            ),
          );
        })),
      ],
    ));
  }
}

/// 弹出菜单里放一张自定义卡片（不是一行菜单项：没有点击效果、不自动关）
class _CardEntry extends PopupMenuEntry<int> {
  final Widget child;
  const _CardEntry({required this.child});

  @override
  double get height => 0;

  @override
  bool represents(int? value) => false;

  @override
  State<_CardEntry> createState() => _CardEntryState();
}

class _CardEntryState extends State<_CardEntry> {
  @override
  Widget build(BuildContext context) => widget.child;
}
