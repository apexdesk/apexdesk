' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 1
' corpus-prints: 300
Sub Main()
    Dim d As Object, t As Long, it As Variant
    Set d = CreateObject("Scripting.Dictionary")
    d("x") = 100: d("y") = 200: d("z") = 300
    d.Remove "x"
    d.Remove "y"
    Debug.Print d.Count
    For Each it In d.Items
        t = t + it
    Next it
    Debug.Print t
End Sub
