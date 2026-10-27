// 首页的登录框。从 dd_home_page.dart 拆出来的，私有类照常互相可见
part of 'dd_home_page.dart';

void _showLogin(BuildContext context) {
  showDialog(context: context, barrierColor: DdColors.scrim, builder: (_) => const _LoginDialog());
}

/// 登录框（照 UU 的登录框：浅蓝渐变底、居中标识、下划线输入框、整宽蓝按钮）。只有域账号口令：
/// 交给网关 /api/login，由网关向 AD 验证；本机不认证、不存口令，只存网关发的令牌和用户信息（和 RustDesk 原登录框一样）
class _LoginDialog extends StatefulWidget {
  const _LoginDialog();

  @override
  State<_LoginDialog> createState() => _LoginDialogState();
}

class _LoginDialogState extends State<_LoginDialog> {
  final _user = TextEditingController(text: bind.mainGetLocalOption(key: _kLastUser));
  final _password = TextEditingController();
  final _passwordFocus = FocusNode();
  bool _busy = false;
  bool _reveal = false;
  String _error = '';

  @override
  void initState() {
    super.initState();
    _user.addListener(() => setState(() {}));
    _password.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _user.dispose();
    _password.dispose();
    _passwordFocus.dispose();
    super.dispose();
  }

  bool get _ready => !_busy && _user.text.trim().isNotEmpty && _password.text.isNotEmpty;

  Future<void> _submit() async {
    if (!_ready) return;
    final user = _user.text.trim();
    setState(() {
      _busy = true;
      _error = '';
    });
    try {
      await ddSignIn(user, _password.text);
      await bind.mainSetLocalOption(key: _kLastUser, value: user);
      if (mounted) Navigator.of(context).pop();
    } on DdSignInError catch (e) {
      _failed(e.message);
    }
  }

  void _failed(String error) {
    if (!mounted) return;
    setState(() {
      _busy = false;
      _error = error;
      _password.clear();
    });
  }

  // 输入框左边一个灰色小图标（照原型：用户名是人像、口令是锁）
  InputDecoration _decoration(String hint, DdIcon icon, {Widget? suffix}) => InputDecoration(
        hintText: hint,
        hintStyle: const TextStyle(color: DdColors.ink3, fontSize: 14),
        isDense: true,
        filled: false,
        prefixIcon: Padding(padding: const EdgeInsets.only(right: 10), child: ddIcon(icon, color: DdColors.ink3)),
        prefixIconConstraints: const BoxConstraints(minWidth: 0, minHeight: 0),
        contentPadding: const EdgeInsets.symmetric(vertical: 12),
        enabledBorder: const UnderlineInputBorder(borderSide: BorderSide(color: DdColors.line)),
        focusedBorder: const UnderlineInputBorder(borderSide: BorderSide(color: DdColors.accent)),
        suffixIcon: suffix,
      );

  @override
  Widget build(BuildContext context) {
    return Dialog(
      backgroundColor: Colors.transparent,
      elevation: 0,
      child: Container(
        width: 420,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(DdRadius.xl),
          // 浅蓝渐变：只此一处用的颜色（照 UU 登录框）
          gradient: const LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            colors: [Color(0xFFDCE8FF), DdColors.surface],
            stops: [0, 0.45],
          ),
          boxShadow: DdShadow.dialog,
        ),
        child: Stack(children: [
          Positioned(
            right: 6,
            top: 6,
            child: IconButton(
              icon: ddIcon(DdIcon.close, color: DdColors.ink2),
              onPressed: () => Navigator.of(context).pop(),
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(44, 40, 44, 22),
            child: Column(mainAxisSize: MainAxisSize.min, crossAxisAlignment: CrossAxisAlignment.stretch, children: [
              Row(mainAxisAlignment: MainAxisAlignment.center, children: [
                loadIcon(30),
                const SizedBox(width: 10),
                const Text('DreamDesk', style: TextStyle(fontSize: 24, fontWeight: FontWeight.w700, color: DdColors.ink)),
              ]),
              const SizedBox(height: 26),
              Text(translate('Sign in with your domain account'),
                  textAlign: TextAlign.center,
                  style: const TextStyle(fontSize: 16, fontWeight: FontWeight.w600, color: DdColors.ink)),
              const SizedBox(height: 14),
              TextField(
                controller: _user,
                autofocus: _user.text.isEmpty,
                style: const TextStyle(fontSize: 14, color: DdColors.ink),
                decoration: _decoration(translate('Username'), DdIcon.user),
                // 用户名里回车：口令还没填就跳到口令框，填了才提交
                onSubmitted: (_) => _password.text.isEmpty ? _passwordFocus.requestFocus() : _submit(),
              ),
              const SizedBox(height: 6),
              TextField(
                controller: _password,
                focusNode: _passwordFocus,
                autofocus: _user.text.isNotEmpty,
                obscureText: !_reveal,
                style: const TextStyle(fontSize: 14, color: DdColors.ink),
                decoration: _decoration(
                  translate('Password'),
                  DdIcon.lock,
                  suffix: IconButton(
                    icon: ddIcon(_reveal ? DdIcon.eye : DdIcon.eyeOff, color: DdColors.ink3),
                    onPressed: () => setState(() => _reveal = !_reveal),
                  ),
                ),
                onSubmitted: (_) => _submit(),
              ),
              SizedBox(
                height: 34,
                child: Align(
                  alignment: Alignment.centerLeft,
                  child: Text(_error, style: const TextStyle(fontSize: 13, color: DdColors.dangerText)),
                ),
              ),
              ElevatedButton(onPressed: _ready ? _submit : null, child: Text(translate(_busy ? 'Loading...' : 'Login'))),
              const SizedBox(height: 28),
              Text('江苏原力数字科技股份有限公司 版权所有 ©${DateTime.now().year}',
                  textAlign: TextAlign.center, style: const TextStyle(fontSize: 12, color: DdColors.ink3)),
            ]),
          ),
        ]),
      ),
    );
  }
}
