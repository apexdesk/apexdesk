' corpus-source: generated
' corpus-flagged: no
' corpus-prints: True
' corpus-prints: False
' corpus-prints: 2
' corpus-prints: a=1;b=2;
Sub Main()
    Dim d As Object, k As Variant, s As String
    Set d = CreateObject("Scripting.Dictionary")
    d.Add "a", 1
    d.Add "b", 2
    Debug.Print d.Exists("a")
    Debug.Print d.Exists("q")
    Debug.Print d.Count
    For Each k In d.Keys
        s = s & k & "=" & d(k) & ";"
    Next k
    Debug.Print s
End Sub
