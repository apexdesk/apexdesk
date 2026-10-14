' corpus-kind: word
' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 2
Sub Main()
    Dim d As Object, w As Variant, t As String
    Set d = CreateObject("Scripting.Dictionary")
    t = LCase(Replace(Replace(ActiveDocument.Content.Text, ".", ""), vbCr, " "))
    For Each w In Split(Trim(t), " ")
        d(w) = d(w) + 1
    Next w
    Debug.Print d("the") - 1
End Sub
