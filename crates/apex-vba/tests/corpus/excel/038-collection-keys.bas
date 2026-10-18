' corpus-source: generated
' corpus-flagged: no
' corpus-prints: 20
' corpus-prints: True
Sub Main()
    Dim col As Collection
    Set col = New Collection
    col.Add 10, "a"
    col.Add 20, "b"
    Debug.Print col("b")
    On Error Resume Next
    Dim x As Variant
    x = col("zzz")
    Debug.Print (Err.Number <> 0)
End Sub
