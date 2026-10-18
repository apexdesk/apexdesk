' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 3
Sub Main()
    Dim t As String
    t = ActiveDocument.Content.Text
    Debug.Print UBound(Split(t, vbCr))
End Sub
