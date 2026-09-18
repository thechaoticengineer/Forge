import QtQuick

// Shared extracted-view button presentation. Callers provide the palette, font and
// padding, so the button stays loadable without the shell modules.
Rectangle {
  id: button

  property string label: ""
  property bool primary: false
  // Deliberately shadow Item.enabled: disabled controls keep their current
  // focus so keyboard escape routing continues to work while activation is gated.
  property bool enabled: true
  required property color foreground
  required property color background
  required property color surface
  required property color accent
  required property string fontFamily
  required property real fontSize
  property color labelColor: primary && enabled ? background : foreground
  property real horizontalPadding: 18
  property real verticalPadding: 10

  signal clicked

  implicitWidth: buttonText.implicitWidth + horizontalPadding
  width: implicitWidth
  height: buttonText.implicitHeight + verticalPadding
  radius: 4
  color: button.primary && button.enabled ? button.accent : button.surface
  border.width: button.primary && button.enabled ? 0 : 1
  border.color: Qt.darker(button.foreground, 3)
  opacity: button.enabled ? (buttonArea.containsMouse ? 0.85 : 1.0) : 0.45

  Text {
    id: buttonText

    anchors.centerIn: parent
    width: Math.max(0, button.width - button.horizontalPadding)
    text: button.label
    textFormat: Text.PlainText
    elide: Text.ElideRight
    color: button.labelColor
    font.family: button.fontFamily
    font.pixelSize: button.fontSize
  }

  MouseArea {
    id: buttonArea

    anchors.fill: parent
    hoverEnabled: true
    enabled: button.enabled
    onClicked: button.clicked()
  }
}
