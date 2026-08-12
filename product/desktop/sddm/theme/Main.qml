import QtQuick 2.15
import QtQuick.Controls 2.15
import SddmComponents 2.0

Rectangle {
    id: root
    width: 1920
    height: 1080
    color: "#0E0E10"

    property string selectedUser: userModel.lastUser
    property int selectedIndex: userModel.lastIndex

    Image {
        anchors.fill: parent
        source: config.background || "/usr/share/wallpapers/TuwaiqOS/contents/images/1920x1080.svg"
        fillMode: Image.PreserveAspectCrop
        asynchronous: true
    }

    Rectangle {
        anchors.fill: parent
        color: "#0E0E10"
        opacity: 0.35
    }

    Column {
        anchors.horizontalCenter: parent.horizontalCenter
        anchors.verticalCenter: parent.verticalCenter
        spacing: 18
        width: 360

        Image {
            anchors.horizontalCenter: parent.horizontalCenter
            width: 72
            height: 72
            source: "/usr/share/tuwaiqos/icons/tuwaiq-mark.svg"
            fillMode: Image.PreserveAspectFit
            asynchronous: true
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: "TUWAIQ OS"
            color: "#F5F2EB"
            font.pixelSize: 28
            font.letterSpacing: 4
            font.family: "Noto Sans"
        }

        Text {
            anchors.horizontalCenter: parent.horizontalCenter
            text: "Sign in"
            color: "#A8A59C"
            font.pixelSize: 14
            font.family: "Noto Sans"
        }

        TextField {
            id: userField
            width: parent.width
            height: 40
            text: root.selectedUser
            color: "#F5F2EB"
            placeholderText: "Username"
            background: Rectangle {
                color: "#1A1A1C"
                border.color: "#C8783A"
                border.width: 1
                radius: 4
            }
            Keys.onPressed: function(event) {
                if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                    passwordField.forceActiveFocus()
            }
        }

        TextField {
            id: passwordField
            width: parent.width
            height: 40
            echoMode: TextInput.Password
            color: "#F5F2EB"
            placeholderText: "Password"
            background: Rectangle {
                color: "#1A1A1C"
                border.color: "#2A4A4E"
                border.width: 1
                radius: 4
            }
            Keys.onPressed: function(event) {
                if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter)
                    loginButton.clicked()
            }
        }

        Button {
            id: loginButton
            width: parent.width
            height: 42
            text: "Login"
            contentItem: Text {
                text: parent.text
                color: "#0E0E10"
                horizontalAlignment: Text.AlignHCenter
                verticalAlignment: Text.AlignVCenter
                font.pixelSize: 15
                font.bold: true
            }
            background: Rectangle {
                color: loginButton.down ? "#A86430" : "#C8783A"
                radius: 4
            }
            onClicked: sddm.login(userField.text, passwordField.text, sessionModel.lastIndex)
        }

        Text {
            id: errorMessage
            anchors.horizontalCenter: parent.horizontalCenter
            color: "#DC5A5A"
            font.pixelSize: 12
            text: ""
            visible: text.length > 0
        }
    }

    Connections {
        target: sddm
        function onLoginFailed() {
            errorMessage.text = "Login failed"
            passwordField.text = ""
            passwordField.forceActiveFocus()
        }
    }

    Component.onCompleted: {
        if (userField.text.length > 0)
            passwordField.forceActiveFocus()
        else
            userField.forceActiveFocus()
    }
}
