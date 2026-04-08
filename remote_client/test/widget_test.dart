import 'package:flutter_test/flutter_test.dart';
import 'package:remote_client/main.dart';

void main() {
  testWidgets('AetherLink app boots', (WidgetTester tester) async {
    await tester.pumpWidget(const RemoteApp());
    expect(find.text('AetherLink'), findsOneWidget);
  });
}
