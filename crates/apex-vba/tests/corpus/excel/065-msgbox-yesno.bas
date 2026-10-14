' corpus-source: generated
' corpus-flagged: no
' corpus-answer: 7
' corpus-prints: no
Sub Main()
    If MsgBox("Continue?", vbYesNo + vbQuestion, "Confirm") = vbYes Then
        Debug.Print "yes"
    Else
        Debug.Print "no"
    End If
End Sub
